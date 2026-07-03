//! Type-signature parser: the human/JSON-facing syntax for `Type`.
//!
//! Grammar (right-associative `->`):
//!   type     := args "->" type | app
//!   args     := app ("," app)*
//!   app      := ident atom+ | atom
//!   atom     := ident | "(" type ")"
//!
//! Identifiers starting lowercase are type variables (`a`, `err`);
//! uppercase are concrete names (`Nat`, `Result`). Dots allowed in
//! names (`Ledger.Entry`).

use crate::Type;

#[derive(Debug, PartialEq)]
pub struct ParseError {
    pub at: usize,
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "type parse error at {}: {}", self.at, self.msg)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Comma,
    Arrow,
    LParen,
    RParen,
}

fn lex(src: &str) -> Result<Vec<(usize, Tok)>, ParseError> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            ' ' | '\t' | '\n' => i += 1,
            ',' => {
                out.push((i, Tok::Comma));
                i += 1;
            }
            '(' => {
                out.push((i, Tok::LParen));
                i += 1;
            }
            ')' => {
                out.push((i, Tok::RParen));
                i += 1;
            }
            '-' if i + 1 < bytes.len() && bytes[i + 1] as char == '>' => {
                out.push((i, Tok::Arrow));
                i += 2;
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < bytes.len() {
                    let c = bytes[i] as char;
                    if c.is_alphanumeric() || c == '_' || c == '.' || c == '\'' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push((start, Tok::Ident(src[start..i].to_string())));
            }
            other => {
                return Err(ParseError {
                    at: i,
                    msg: format!("unexpected character `{other}`"),
                })
            }
        }
    }
    Ok(out)
}

struct P {
    toks: Vec<(usize, Tok)>,
    pos: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(_, t)| t)
    }

    fn at(&self) -> usize {
        self.toks
            .get(self.pos)
            .map(|(i, _)| *i)
            .unwrap_or(usize::MAX)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|(_, t)| t.clone());
        self.pos += 1;
        t
    }

    fn ident_to_type(name: &str) -> Type {
        let first = name.chars().next().unwrap_or('_');
        if first.is_lowercase() {
            Type::Var(name.to_string())
        } else {
            Type::Named(name.to_string())
        }
    }

    /// atom := ident | "(" type ")"
    fn atom(&mut self) -> Result<Type, ParseError> {
        match self.bump() {
            Some(Tok::Ident(n)) => Ok(Self::ident_to_type(&n)),
            Some(Tok::LParen) => {
                let t = self.ty()?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(t),
                    _ => Err(ParseError {
                        at: self.at(),
                        msg: "expected `)`".into(),
                    }),
                }
            }
            other => Err(ParseError {
                at: self.at(),
                msg: format!("expected type, found {other:?}"),
            }),
        }
    }

    /// app := ident atom+ | atom
    fn app(&mut self) -> Result<Type, ParseError> {
        let head = self.atom()?;
        // application only valid when head is a bare (uppercase) name
        let mut args = Vec::new();
        while matches!(self.peek(), Some(Tok::Ident(_)) | Some(Tok::LParen)) {
            args.push(self.atom()?);
        }
        if args.is_empty() {
            return Ok(head);
        }
        match head {
            Type::Named(h) => Ok(Type::App(h, args)),
            other => Err(ParseError {
                at: self.at(),
                msg: format!("`{other}` cannot be applied to arguments"),
            }),
        }
    }

    /// type := args "->" type | app
    fn ty(&mut self) -> Result<Type, ParseError> {
        let first = self.app()?;
        let mut args = vec![first];
        while matches!(self.peek(), Some(Tok::Comma)) {
            self.bump();
            args.push(self.app()?);
        }
        if matches!(self.peek(), Some(Tok::Arrow)) {
            self.bump();
            let ret = self.ty()?; // right-associative
            return Ok(Type::Fn(args, Box::new(ret)));
        }
        if args.len() == 1 {
            Ok(args.pop().unwrap())
        } else {
            Err(ParseError {
                at: self.at(),
                msg: "comma-separated types must be followed by `->`".into(),
            })
        }
    }
}

/// Parse a type signature like `Nat, Nat -> Result Nat MathErr`.
pub fn parse_type(src: &str) -> Result<Type, ParseError> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err(ParseError {
            at: 0,
            msg: "empty type".into(),
        });
    }
    let mut p = P { toks, pos: 0 };
    let t = p.ty()?;
    if p.pos != p.toks.len() {
        return Err(ParseError {
            at: p.at(),
            msg: "trailing tokens".into(),
        });
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    #[test]
    fn parses_atoms() {
        assert_eq!(parse_type("Nat").unwrap(), named("Nat"));
        assert_eq!(parse_type("a").unwrap(), Type::Var("a".into()));
        assert_eq!(parse_type("Ledger.Entry").unwrap(), named("Ledger.Entry"));
    }

    #[test]
    fn parses_application() {
        assert_eq!(
            parse_type("List Nat").unwrap(),
            Type::App("List".into(), vec![named("Nat")])
        );
        assert_eq!(
            parse_type("Result Nat (List a)").unwrap(),
            Type::App(
                "Result".into(),
                vec![
                    named("Nat"),
                    Type::App("List".into(), vec![Type::Var("a".into())])
                ]
            )
        );
    }

    #[test]
    fn parses_functions_multi_arg() {
        assert_eq!(
            parse_type("Nat, Nat -> Result Nat MathErr").unwrap(),
            Type::Fn(
                vec![named("Nat"), named("Nat")],
                Box::new(Type::App(
                    "Result".into(),
                    vec![named("Nat"), named("MathErr")]
                ))
            )
        );
    }

    #[test]
    fn arrow_is_right_associative() {
        assert_eq!(
            parse_type("Nat -> Nat -> Nat").unwrap(),
            Type::Fn(
                vec![named("Nat")],
                Box::new(Type::Fn(vec![named("Nat")], Box::new(named("Nat"))))
            )
        );
    }

    #[test]
    fn parens_override() {
        assert_eq!(
            parse_type("(Nat -> Nat) -> Nat").unwrap(),
            Type::Fn(
                vec![Type::Fn(vec![named("Nat")], Box::new(named("Nat")))],
                Box::new(named("Nat"))
            )
        );
    }

    #[test]
    fn roundtrips_display() {
        for s in [
            "Nat",
            "List Nat",
            "Nat, Nat -> Result Nat MathErr",
            "Account, Nat -> Result Ledger Err",
            "(a -> a), a -> a",
        ] {
            let t = parse_type(s).unwrap();
            assert_eq!(parse_type(&t.to_string()).unwrap(), t, "roundtrip {s}");
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_type("").is_err());
        assert!(parse_type("Nat,").is_err());
        assert!(parse_type("Nat, Nat").is_err(), "comma without arrow");
        assert!(parse_type("-> Nat").is_err());
        assert!(parse_type("a Nat").is_err(), "var cannot be applied");
    }
}
