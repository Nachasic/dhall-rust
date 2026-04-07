#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
use std::path::Path;
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
use std::path::PathBuf;
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
use url::Url;

use crate::error::Error;
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
use crate::error::ImportError;
use crate::semantics::resolve::ImportLocation;
use crate::syntax::{binary, parse_expr};
use crate::Parsed;

#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
fn resolve_home(path: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let mut f = PathBuf::new();
    match path.as_ref().strip_prefix("~") {
        Ok(rest) => {
            let home = home::home_dir()
                .ok_or_else(|| Error::from(ImportError::MissingHome))?;
            f.push(home);
            f.push(rest);
        }
        Err(_) => {
            f.push(path);
        }
    }
    Ok(f)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "std", feature = "reqwest"))]
fn download_http_text(url: Url) -> Result<String, Error> {
    Ok(reqwest::blocking::get(url).unwrap().text().unwrap())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "std", not(feature = "reqwest")))]
fn download_http_text(_url: Url) -> Result<String, Error> {
    panic!("Remote imports are disabled in this build of dhall-rust")
}

#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
pub fn parse_file(f: &Path) -> Result<Parsed, Error> {
    let path = resolve_home(f)?;
    let text = std::fs::read_to_string(path)?;
    let expr = parse_expr(&text)?;
    let root = ImportLocation::local_dhall_code(f.to_owned());
    Ok(Parsed(expr, root))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
pub fn parse_remote(url: Url) -> Result<Parsed, Error> {
    let body = download_http_text(url.clone())?;
    let expr = parse_expr(&body)?;
    let root = ImportLocation::remote_dhall_code(url);
    Ok(Parsed(expr, root))
}

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

#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
pub fn parse_binary_file(f: &Path) -> Result<Parsed, Error> {
    let data = crate::utils::read_binary_file(f)?;
    let expr = binary::decode(&data)?;
    let root = ImportLocation::local_dhall_code(f.to_owned());
    Ok(Parsed(expr, root))
}
