//! Extensible evaluation engine for Dhall.
//!
//! Wraps the `dhall` crate to support custom builtin functions that
//! participate in normalization, and custom import resolution.

pub mod conv;

use std::sync::Arc;

use dhall::ctxt::{CustomBuiltinEntry, CustomBuiltinHandler};
use dhall::semantics::{ImportFetcher, Nir, NirKind};
use dhall::syntax::Label;
use dhall::{Ctxt, Parsed};

/// Re-export core dhall types needed by engine users.
pub mod types {
    pub use dhall::builtins::Builtin;
    pub use dhall::ctxt::CustomBuiltinHandler;
    pub use dhall::operations::OpKind;
    pub use dhall::semantics::{ImportFetcher, ImportLocation, ImportLocationKind, LocalPath, Nir, NirKind, TextLit};
    pub use dhall::syntax::{Expr, ExprKind, ImportMode, Label, NumKind, Span};
    pub use dhall::{Ctxt, Normalized, Parsed, Resolved, Typed};
    pub use crate::conv::{DhallType, FromNir, IntoNir, NirExt, NirRecord, NirRecordBuilder};
}

// ── Engine ───────────────────────────────────────────────────────────

pub struct Engine {
    fetcher: Option<Arc<dyn for<'cx> ImportFetcher>>,
    builtins: Vec<CustomBuiltinEntry>,
}

impl Engine {
    pub fn new() -> Self {
        Engine { fetcher: None, builtins: Vec::new() }
    }

    /// Set a custom import fetcher. Receives the full `ImportLocation`
    /// for each import — return `Some(Ok(source))` to provide content,
    /// `Some(Err(...))` to fail, or `None` to fall back to default I/O.
    pub fn with_fetcher(mut self, fetcher: impl ImportFetcher + 'static) -> Self {
        self.fetcher = Some(Arc::new(fetcher));
        self
    }

    pub fn with_builtin(
        mut self,
        name: &str,
        handler: impl for<'cx> CustomBuiltinHandler<'cx> + 'static,
    ) -> Self {
        self.builtins.push(CustomBuiltinEntry {
            name: name.to_owned(),
            handler: Arc::new(handler),
        });
        self
    }

    pub fn eval_str(&self, input: &str) -> Result<dhall::syntax::Expr, dhall::error::Error> {
        if self.builtins.is_empty() {
            return Ctxt::with_new(|cx| {
                let nir = self.resolve_and_normalize(cx, Parsed::parse_str(input)?)?;
                Ok(nir.to_expr(cx, Default::default()))
            });
        }

        let entries = self.builtins.clone();

        Ctxt::with_new_custom(entries, |cx| {
            let parsed = Parsed::parse_str(input)?;
            let resolved = self.resolve(cx, parsed)?;
            let expr = resolved.to_expr(cx);

            // Build Hir with builtin names in scope.
            let mut name_env = dhall::semantics::NameEnv::new();
            for b in &self.builtins {
                name_env.insert_mut(&Label::from(b.name.as_str()));
            }
            let hir = dhall::expr_to_hir(&expr, &mut name_env);

            // Build NzEnv with CustomBuiltin values at matching indices.
            let mut nz_env = dhall::semantics::NzEnv::new(cx);
            for (i, _) in self.builtins.iter().enumerate() {
                nz_env = nz_env.insert_value(
                    Nir::from_kind(NirKind::CustomBuiltin(cx, i, Vec::new())),
                    (),
                );
            }

            let nir = hir.eval(nz_env);
            Ok(nir.to_expr(cx, Default::default()))
        })
    }

    fn resolve<'cx>(&self, cx: Ctxt<'cx>, p: Parsed) -> Result<dhall::Resolved<'cx>, dhall::error::Error> {
        match &self.fetcher {
            Some(f) => p.resolve_with_fetcher(cx, Box::new(ArcFetcher(Arc::clone(f)))),
            None => p.resolve(cx),
        }
    }

    fn resolve_and_normalize<'cx>(&self, cx: Ctxt<'cx>, p: Parsed) -> Result<Nir<'cx>, dhall::error::Error> {
        Ok(self.resolve(cx, p)?.typecheck(cx)?.normalize(cx).as_nir().clone())
    }
}

impl Default for Engine {
    fn default() -> Self { Self::new() }
}

/// Wrapper to clone an Arc<dyn ImportFetcher> into a Box<dyn ImportFetcher>.
struct ArcFetcher(Arc<dyn for<'cx> ImportFetcher>);

impl ImportFetcher for ArcFetcher {
    fn chain(&self, base: &dhall::semantics::ImportLocation, import: &dhall::semantics::Import) -> Option<Result<dhall::semantics::ImportLocation, dhall::error::Error>> {
        self.0.chain(base, import)
    }
    fn fetch(&self, location: &dhall::semantics::ImportLocation) -> Option<Result<String, dhall::error::Error>> {
        self.0.fetch(location)
    }
}

/// A fetcher that rejects all imports. Useful for sandboxed evaluation.
pub struct NoImports;

impl ImportFetcher for NoImports {
    fn fetch(&self, _location: &dhall::semantics::ImportLocation) -> Option<Result<String, dhall::error::Error>> {
        Some(Err(dhall::error::Error::from(dhall::error::ImportError::Missing)))
    }
}
