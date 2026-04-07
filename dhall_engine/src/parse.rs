//! I/O-based parsing entry points: parse from files, URLs, binary files.

#[cfg(feature = "std")]
use std::path::Path;

use dhall::error::Error;
use dhall::Parsed;

/// Parse a Dhall source file from disk.
#[cfg(feature = "std")]
pub fn parse_file(f: &Path) -> Result<Parsed, Error> {
    let path = crate::fetcher::resolve_home(f)?;
    let text = std::fs::read_to_string(&path)?;
    let expr = dhall::syntax::parse_expr(&text)?;
    let root = dhall::semantics::ImportLocation::local_dhall_code(f.to_owned());
    Ok(Parsed::from_expr(expr, root))
}

/// Parse a Dhall binary file from disk.
#[cfg(feature = "std")]
pub fn parse_binary_file(f: &Path) -> Result<Parsed, Error> {
    let data = dhall::utils::read_binary_file(f)?;
    let expr = dhall::syntax::binary::decode(&data)?;
    let root = dhall::semantics::ImportLocation::local_dhall_code(f.to_owned());
    Ok(Parsed::from_expr(expr, root))
}

/// Parse Dhall source fetched from a remote URL.
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
pub fn parse_remote(url: url::Url) -> Result<Parsed, Error> {
    let body = crate::fetcher::download_http_text(url.clone())?;
    let expr = dhall::syntax::parse_expr(&body)?;
    let root = dhall::semantics::ImportLocation::remote_dhall_code(url);
    Ok(Parsed::from_expr(expr, root))
}
