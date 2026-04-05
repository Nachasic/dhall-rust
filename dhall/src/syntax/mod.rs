#![allow(
    clippy::many_single_char_names,
    clippy::should_implement_trait,
    clippy::new_without_default,
    clippy::type_complexity
)]

mod ast;
pub use crate::syntax::ast::visitor;
pub use crate::syntax::ast::*;
// Default: nom parser.
pub use crate::syntax::text::nom_parser::{parse_expr, ParseError};
pub mod binary;
pub mod text;
