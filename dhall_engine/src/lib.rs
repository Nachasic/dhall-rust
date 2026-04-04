//! Extensible evaluation engine for Dhall.
//!
//! Wraps the `dhall` crate to support custom builtin functions that
//! participate in normalization.
//!
//! Custom builtins implement [`CustomBuiltinHandler`] (from `dhall`) and are
//! registered by name. During normalization, when a builtin is applied to
//! arguments, Dhall dispatches to the Rust callback. Results feed back into
//! normalization — field access, arithmetic, conditionals all work.

pub mod conv;
pub mod resolve;

use std::sync::Arc;

use dhall::ctxt::{CustomBuiltinEntry, CustomBuiltinHandler};
use dhall::semantics::{Nir, NirKind};
use dhall::syntax::Label;
use dhall::{Ctxt, Parsed};

/// Re-export core dhall types needed by engine users.
pub mod types {
    pub use dhall::builtins::Builtin;
    pub use dhall::ctxt::CustomBuiltinHandler;
    pub use dhall::operations::OpKind;
    pub use dhall::semantics::{Nir, NirKind, TextLit};
    pub use dhall::syntax::{Expr, ExprKind, Label, NumKind, Span};
    pub use dhall::{Ctxt, Normalized, Parsed, Resolved, Typed};
    pub use crate::conv::{DhallType, FromNir, IntoNir, NirExt, NirRecord, NirRecordBuilder};
}

// ── Engine ───────────────────────────────────────────────────────────

pub struct Engine<R = resolve::DefaultResolver> {
    resolver: R,
    builtins: Vec<CustomBuiltinEntry>,
}

impl Engine {
    pub fn new() -> Self {
        Engine { resolver: resolve::DefaultResolver, builtins: Vec::new() }
    }
}

impl Default for Engine {
    fn default() -> Self { Self::new() }
}

impl<R> Engine<R> {
    pub fn with_resolver<R2>(self, resolver: R2) -> Engine<R2> {
        Engine { resolver, builtins: self.builtins }
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
}

impl<R: resolve::ImportResolver> Engine<R> {
    pub fn eval_str(&self, input: &str) -> Result<dhall::syntax::Expr, dhall::error::Error> {
        if self.builtins.is_empty() {
            return Ctxt::with_new(|cx| {
                let nir = self.run_pipeline(cx, Parsed::parse_str(input)?)?;
                Ok(nir.to_expr(cx, Default::default()))
            });
        }

        // Clone entries (Arc makes this cheap) for with_new_custom which takes by value.
        let entries = self.builtins.clone();

        Ctxt::with_new_custom(entries, |cx| {
            // Resolve user source normally (builtin names will be free variables).
            let parsed = Parsed::parse_str(input)?;
            let resolved = self.resolver.resolve(cx, parsed)?;
            let expr = resolved.to_expr(cx);

            // Build Hir with builtin names in scope as proper variables.
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

    fn run_pipeline<'cx>(&self, cx: Ctxt<'cx>, p: Parsed) -> Result<Nir<'cx>, dhall::error::Error> {
        Ok(self.resolver.resolve(cx, p)?.typecheck(cx)?.normalize(cx).as_nir().clone())
    }
}
