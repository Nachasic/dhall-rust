use crate::error::Error;
use crate::semantics::resolve::ImportLocation;
use crate::syntax::{binary, parse_expr};
use crate::Parsed;

pub fn parse_str(s: &str) -> Result<Parsed, Error> {
    let expr = parse_expr(s)?;
    let root = ImportLocation::dhall_code_of_unknown_origin();
    Ok(Parsed(expr, root))
}

pub fn parse_binary(data: &[u8]) -> Result<Parsed, Error> {
    let expr = binary::decode(data)?;
    let root = ImportLocation::dhall_code_of_unknown_origin();
    Ok(Parsed(expr, root))
}
