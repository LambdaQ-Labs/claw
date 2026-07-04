//! claw-contract — executable contracts (WS-E).
//!
//! Contracts (`requires` / `ensures` / `example`) are not comments: they
//! are a checkable predicate language. This crate parses the predicate
//! strings the benchmark tasks already carry (e.g. "result >= lo",
//! "ok(result) => from'.balance == from.balance - amt"), evaluates them
//! against a value environment, and generates property test cases.
//!
//! This is the layer that catches "compiles but does the wrong thing":
//! given inputs and a produced output, a postcondition is a boolean you
//! can actually run. The full story routes through the compiler's
//! interpreter; this crate makes the *spec* executable now.
//!
//! Spec: docs/syntax.md §2, master-plan WS-E.

mod eval;
mod parse;
mod property;

pub use eval::{eval_pred, Value};
pub use parse::{parse_pred, ParseError};
pub use property::{generate_cases, generate_from_strings, Case};

use serde::{Deserialize, Serialize};

/// A contract attached to a definition.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Contract {
    /// Preconditions — must hold on inputs before the call.
    pub requires: Vec<String>,
    /// Postconditions — must hold on (inputs, result) after.
    pub ensures: Vec<String>,
    /// Concrete input→output examples.
    pub examples: Vec<String>,
}

/// Comparison / boolean operators in the predicate language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Eq,
    Ne,
    Le,
    Lt,
    Ge,
    Gt,
}

/// Arithmetic in predicate value expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arith {
    Add,
    Sub,
    Mul,
}

/// Predicate value expressions: `from.balance`, `amt`, `x - y`, `f(x)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PExpr {
    /// Variable, possibly with post-state prime and field path:
    /// `from`, `from'`, `from.balance`, `from'.balance`.
    Var(String),
    Int(i64),
    Bin(Arith, Box<PExpr>, Box<PExpr>),
    /// A pure call named in scope: `List.len(result)`.
    Call(String, Vec<PExpr>),
}

/// Predicates: comparisons, boolean combinators, implication, and the
/// `ok(result)` / `err(result)` result-tag guards used in ensures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pred {
    Cmp(Op, PExpr, PExpr),
    And(Box<Pred>, Box<Pred>),
    Or(Box<Pred>, Box<Pred>),
    Not(Box<Pred>),
    Implies(Box<Pred>, Box<Pred>),
    /// `ok(x)` — true iff the named value is a successful Result.
    IsOk(String),
    /// `err(x)` — true iff the named value is an error Result.
    IsErr(String),
    Bool(bool),
}
