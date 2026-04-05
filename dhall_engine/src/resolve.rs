//! Import resolution strategies.

use dhall::error::Error;
use dhall::{Ctxt, Parsed, Resolved};

/// Controls how Dhall imports are resolved.
pub trait ImportResolver {
    fn resolve<'cx>(&self, cx: Ctxt<'cx>, parsed: Parsed) -> Result<Resolved<'cx>, Error>;
}

/// Standard resolver: filesystem + HTTP.
pub struct DefaultResolver;
impl ImportResolver for DefaultResolver {
    fn resolve<'cx>(&self, cx: Ctxt<'cx>, p: Parsed) -> Result<Resolved<'cx>, Error> { p.resolve(cx) }
}

/// Rejects all imports. Useful for sandboxed evaluation.
pub struct NoImports;
impl ImportResolver for NoImports {
    fn resolve<'cx>(&self, cx: Ctxt<'cx>, p: Parsed) -> Result<Resolved<'cx>, Error> { p.skip_resolve(cx) }
}
