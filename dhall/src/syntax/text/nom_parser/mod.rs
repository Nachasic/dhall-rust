//! Dhall parser built on `nom`.
//!
//! Follows the [Dhall ABNF grammar](https://github.com/dhall-lang/dhall-lang/blob/master/standard/dhall.abnf)
//! and produces the `Expr` AST.
//!
//! # Structure
//!
//! Submodules are organized bottom-up:
//! 1. `input` — Custom Input type and nom trait implementations
//! 2. `helpers` — Shared types, error constructors, whitespace
//! 3. `literals` — Numbers, strings
//! 4. `labels` — Labels, variables, builtins
//! 5. `imports` — File, HTTP, env, missing imports
//! 6. `structure` — Atoms, records, unions, lists
//! 7. `application` — Selectors, completion, application
//! 8. `operators` — Precedence tower
//! 9. `expression` — Top-level expressions (let, lambda, if, etc.)
//! 10. `errors` — Error formatting and diagnostics

mod input;
mod helpers;
mod literals;
mod labels;
mod imports;
mod structure;
mod application;
mod operators;
mod expression;
mod errors;
#[cfg(test)]
mod tests;

pub use helpers::ParseError;
pub use expression::parse_expr;
