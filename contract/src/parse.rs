//! Recursive-descent parser for the predicate language.
//!
//! Grammar (loosest → tightest binding):
//!   pred    := implies
//!   implies := or ("=>" implies)?
//!   or      := and ("||" and)*
//!   and     := cmp ("&&" cmp)*
//!   cmp     := sum (op sum)? | "ok(" name ")" | "err(" name ")" | "(" pred ")"
//!   sum     := term (("+"|"-") term)*
//!   term    := atom (("*") atom)*
//!   atom    := int | path | call | "(" sum ")"
//!   path    := name ("'")? ("." name)*
//!   call    := name "(" (sum ("," sum)*)? ")"

use crate::{Arith, Op, PExpr, Pred};

#[derive(Debug, PartialEq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "predicate parse error: {}", self.0)
    }
}
impl std::error::Error for ParseError {}

pub fn parse_pred(src: &str) -> Result<Pred, ParseError> {
    let toks = lex(src)?;
    let mut p = P { toks, i: 0 };
    let pred = p.implies()?;
    if p.i != p.toks.len() {
        return Err(ParseError(format!("trailing tokens at {}", p.i)));
    }
    Ok(pred)
}

#[derive(Debug, Clone, PartialEq)]
enum T {
    Name(String),
    Int(i64),
    Op(String), // multi-char & single-char operators / punctuation
}

fn lex(s: &str) -> Result<Vec<T>, ParseError> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' | '\n' => i += 1,
            '0'..='9' => {
                let st = i;
                while i < b.len() && (b[i] as char).is_ascii_digit() {
                    i += 1;
                }
                out.push(T::Int(
                    s[st..i].parse().map_err(|_| ParseError("bad int".into()))?,
                ));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let st = i;
                while i < b.len()
                    && matches!(b[i] as char, 'a'..='z'|'A'..='Z'|'0'..='9'|'_'|'.'|'\'')
                {
                    i += 1;
                }
                out.push(T::Name(s[st..i].to_string()));
            }
            _ => {
                // multi-char operators first
                let two = if i + 1 < b.len() { &s[i..i + 2] } else { "" };
                match two {
                    "==" | "!=" | "<=" | ">=" | "=>" | "&&" | "||" => {
                        out.push(T::Op(two.to_string()));
                        i += 2;
                    }
                    _ => {
                        if "()+-*<>,".contains(c) {
                            out.push(T::Op(c.to_string()));
                            i += 1;
                        } else {
                            return Err(ParseError(format!("unexpected char `{c}`")));
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

struct P {
    toks: Vec<T>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<&T> {
        self.toks.get(self.i)
    }
    fn eat_op(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(T::Op(o)) if o == op) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn implies(&mut self) -> Result<Pred, ParseError> {
        let lhs = self.or()?;
        if self.eat_op("=>") {
            let rhs = self.implies()?;
            return Ok(Pred::Implies(Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn or(&mut self) -> Result<Pred, ParseError> {
        let mut lhs = self.and()?;
        while self.eat_op("||") {
            lhs = Pred::Or(Box::new(lhs), Box::new(self.and()?));
        }
        Ok(lhs)
    }

    fn and(&mut self) -> Result<Pred, ParseError> {
        let mut lhs = self.cmp()?;
        while self.eat_op("&&") {
            lhs = Pred::And(Box::new(lhs), Box::new(self.cmp()?));
        }
        Ok(lhs)
    }

    fn cmp(&mut self) -> Result<Pred, ParseError> {
        // ok(x) / err(x) guards
        if let Some(T::Name(n)) = self.peek() {
            if (n == "ok" || n == "err")
                && matches!(self.toks.get(self.i + 1), Some(T::Op(o)) if o == "(")
            {
                let is_ok = n == "ok";
                self.i += 2; // name + "("
                let arg = match self.peek() {
                    Some(T::Name(a)) => a.clone(),
                    _ => return Err(ParseError("expected name in ok()/err()".into())),
                };
                self.i += 1;
                if !self.eat_op(")") {
                    return Err(ParseError("expected )".into()));
                }
                return Ok(if is_ok {
                    Pred::IsOk(arg)
                } else {
                    Pred::IsErr(arg)
                });
            }
        }
        if self.eat_op("(") {
            // could be a parenthesized predicate
            let save = self.i;
            if let Ok(p) = self.implies() {
                if self.eat_op(")") {
                    return Ok(p);
                }
            }
            self.i = save; // fall back: it was a parenthesized sum in a comparison
            self.i -= 1; // un-eat "("
        }
        let lhs = self.sum()?;
        let op = match self.peek() {
            Some(T::Op(o)) => match o.as_str() {
                "==" => Op::Eq,
                "!=" => Op::Ne,
                "<=" => Op::Le,
                "<" => Op::Lt,
                ">=" => Op::Ge,
                ">" => Op::Gt,
                _ => return Err(ParseError(format!("expected comparison, got {o}"))),
            },
            _ => return Err(ParseError("expected comparison operator".into())),
        };
        self.i += 1;
        let rhs = self.sum()?;
        Ok(Pred::Cmp(op, lhs, rhs))
    }

    fn sum(&mut self) -> Result<PExpr, ParseError> {
        let mut lhs = self.term()?;
        loop {
            if self.eat_op("+") {
                lhs = PExpr::Bin(Arith::Add, Box::new(lhs), Box::new(self.term()?));
            } else if self.eat_op("-") {
                lhs = PExpr::Bin(Arith::Sub, Box::new(lhs), Box::new(self.term()?));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<PExpr, ParseError> {
        let mut lhs = self.atom()?;
        while self.eat_op("*") {
            lhs = PExpr::Bin(Arith::Mul, Box::new(lhs), Box::new(self.atom()?));
        }
        Ok(lhs)
    }

    fn atom(&mut self) -> Result<PExpr, ParseError> {
        match self.peek().cloned() {
            Some(T::Int(n)) => {
                self.i += 1;
                Ok(PExpr::Int(n))
            }
            Some(T::Name(n)) => {
                self.i += 1;
                if self.eat_op("(") {
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(T::Op(o)) if o == ")") {
                        args.push(self.sum()?);
                        while self.eat_op(",") {
                            args.push(self.sum()?);
                        }
                    }
                    if !self.eat_op(")") {
                        return Err(ParseError("expected ) after call args".into()));
                    }
                    Ok(PExpr::Call(n, args))
                } else {
                    Ok(PExpr::Var(n))
                }
            }
            Some(T::Op(o)) if o == "(" => {
                self.i += 1;
                let e = self.sum()?;
                if !self.eat_op(")") {
                    return Err(ParseError("expected )".into()));
                }
                Ok(e)
            }
            other => Err(ParseError(format!("expected value, got {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_comparison() {
        assert_eq!(
            parse_pred("result >= lo").unwrap(),
            Pred::Cmp(Op::Ge, PExpr::Var("result".into()), PExpr::Var("lo".into()))
        );
    }

    #[test]
    fn parses_implication_with_prime_and_field() {
        let p = parse_pred("ok(result) => from'.balance == from.balance - amt").unwrap();
        match p {
            Pred::Implies(l, r) => {
                assert_eq!(*l, Pred::IsOk("result".into()));
                assert!(matches!(
                    *r,
                    Pred::Cmp(Op::Eq, PExpr::Var(_), PExpr::Bin(Arith::Sub, ..))
                ));
            }
            _ => panic!("expected implication"),
        }
    }

    #[test]
    fn parses_call_and_and() {
        let p = parse_pred("List.len(result) == List.len(input) + 1").unwrap();
        assert!(matches!(
            p,
            Pred::Cmp(Op::Eq, PExpr::Call(_, _), PExpr::Bin(Arith::Add, ..))
        ));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_pred("").is_err());
        assert!(parse_pred("result >").is_err());
        assert!(parse_pred(">= lo").is_err());
    }
}
