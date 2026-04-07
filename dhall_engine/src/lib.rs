//! Extensible evaluation engine for Dhall.
//!
//! Wraps the `dhall` crate to support custom builtin functions that
//! participate in normalization, and custom import resolution.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod conv;

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dhall::ctxt::{CustomBuiltinEntry, CustomBuiltinHandler};
use dhall::semantics::{Hir, HirKind, ImportFetcher, Nir, NirKind, NzEnv, TyEnv, type_with};
use dhall::syntax::{ExprKind, Label};
use dhall::{Ctxt, Parsed};

/// Re-export core dhall types needed by engine users.
pub mod types {
    pub use dhall::builtins::Builtin;
    pub use dhall::ctxt::CustomBuiltinHandler;
    pub use dhall::operations::OpKind;
    pub use dhall::semantics::{Hir, HirKind, ImportFetcher, ImportLocation, ImportLocationKind, LocalPath, Nir, NirKind, TextLit};
    pub use dhall::syntax::{Expr, ExprKind, ImportMode, Label, NumKind, Span};
    pub use dhall::{Ctxt, Normalized, Parsed, Resolved, Typed};
    pub use crate::conv::{DhallType, FromNir, IntoNir, NirExt, NirRecord, NirRecordBuilder};
    pub use crate::Lazy;
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

    /// Register a custom builtin with a Dhall type signature.
    ///
    /// The `type_sig` must be a valid Dhall type expression, e.g.:
    /// - `"Natural -> Natural"`
    /// - `"{ name : Text, src : Text } -> { hash : Text, name : Text }"`
    /// - `"forall (a : Type) -> List a -> Natural"`
    pub fn with_builtin(
        mut self,
        name: &str,
        type_sig: &str,
        handler: impl for<'cx> CustomBuiltinHandler<'cx> + 'static,
    ) -> Self {
        self.builtins.push(CustomBuiltinEntry {
            name: name.to_owned(),
            type_sig: type_sig.to_owned(),
            handler: Arc::new(handler),
        });
        self
    }

    pub fn eval_str(&self, input: &str) -> Result<dhall::syntax::Expr, dhall::error::Error> {
        let entries = self.builtins.clone();

        Ctxt::with_new_custom(entries, |cx| {
            let parsed = Parsed::parse_str(input)?;
            let name_env = self.build_name_env();
            let resolved = self.resolve_with_names(cx, parsed, &name_env)?;

            if self.builtins.is_empty() {
                let typed = resolved.typecheck(cx)?;
                return Ok(typed.normalize(cx).to_expr(cx));
            }

            let ty_env = self.build_ty_env(cx)?;
            let typed = resolved.typecheck_with_env(&ty_env)?;
            let nir = typed.eval_to_nir(&ty_env.to_nzenv());
            Ok(nir.to_expr(cx, Default::default()))
        })
    }

    fn build_name_env(&self) -> dhall::semantics::NameEnv {
        let mut env = dhall::semantics::NameEnv::new();
        for b in &self.builtins {
            env.insert_mut(&Label::from(b.name.as_str()));
        }
        env
    }

    fn build_ty_env<'cx>(&self, cx: Ctxt<'cx>) -> Result<TyEnv<'cx>, dhall::error::Error> {
        let mut ty_env = TyEnv::new(cx);
        for (i, b) in self.builtins.iter().enumerate() {
            let type_hir = Parsed::parse_str(&b.type_sig)?
                .skip_resolve(cx)?
                .typecheck(cx)?
                .as_hir()
                .clone();
            let ty = type_with(&ty_env, &type_hir, None)?.eval_to_type(&ty_env)?;
            let nir = Nir::from_kind(NirKind::CustomBuiltin(cx, i, Vec::new()));
            ty_env = ty_env.insert_value(&Label::from(b.name.as_str()), nir, ty);
        }
        Ok(ty_env)
    }

    fn resolve_with_names<'cx>(
        &self,
        cx: Ctxt<'cx>,
        p: Parsed,
        names: &dhall::semantics::NameEnv,
    ) -> Result<dhall::Resolved<'cx>, dhall::error::Error> {
        match &self.fetcher {
            Some(f) => p.resolve_with_names_and_fetcher(cx, names, Box::new(ArcFetcher(Arc::clone(f)))),
            None => p.resolve_with_names(cx, names),
        }
    }
}

impl Default for Engine {
    fn default() -> Self { Self::new() }
}

// ── Lazy evaluation ──────────────────────────────────────────────────

/// A lazy handle to a resolved, typechecked Dhall expression.
/// Fields and sub-expressions are only normalized on demand.
pub struct Lazy<'cx> {
    hir: Hir<'cx>,
    env: NzEnv<'cx>,
    cx: Ctxt<'cx>,
}

impl<'cx> Lazy<'cx> {
    /// Fully normalize this expression and return the resulting `Nir`.
    pub fn normalize(&self) -> Nir<'cx> {
        self.hir.eval(&self.env)
    }

    /// Fully normalize and convert to an `Expr`.
    pub fn to_expr(&self) -> dhall::syntax::Expr {
        self.normalize().to_expr(self.cx, Default::default())
    }

    /// If this is a record literal, return the field names without evaluating anything.
    pub fn field_names(&self) -> Option<Vec<String>> {
        match self.hir.kind() {
            HirKind::Expr(ExprKind::RecordLit(fields)) => {
                Some(fields.keys().map(|l| l.as_ref().to_owned()).collect())
            }
            _ => None,
        }
    }

    /// If this is a record literal, get a lazy handle to a single field.
    /// Returns `None` if not a record or the field doesn't exist.
    pub fn field(&self, name: &str) -> Option<Lazy<'cx>> {
        match self.hir.kind() {
            HirKind::Expr(ExprKind::RecordLit(fields)) => {
                let hir = fields.get(&Label::from(name))?.clone();
                Some(Lazy { hir, env: self.env.clone(), cx: self.cx })
            }
            _ => None,
        }
    }

    /// Access the underlying `Hir` for advanced traversal.
    pub fn as_hir(&self) -> &Hir<'cx> {
        &self.hir
    }

    /// Access the normalization environment.
    pub fn env(&self) -> &NzEnv<'cx> {
        &self.env
    }

    /// Access the context.
    pub fn cx(&self) -> Ctxt<'cx> {
        self.cx
    }
}

impl Engine {
    /// Parse, resolve, and typecheck the input, then hand a [`Lazy`] handle
    /// to the callback. Nothing is normalized until you explicitly ask for it.
    ///
    /// The `Lazy` handle is scoped to the callback because the evaluation
    /// context (`Ctxt`) cannot outlive it.
    ///
    /// ```ignore
    /// engine.eval_lazy("{ a = 1, b = doubleNat 21 }", |lazy| {
    ///     let names = lazy.field_names().unwrap();
    ///     let a = lazy.field("a").unwrap().to_expr();
    ///     // `doubleNat` is never called unless we touch field "b"
    /// });
    /// ```
    pub fn eval_lazy<T>(
        &self,
        input: &str,
        f: impl FnOnce(&Lazy<'_>) -> T,
    ) -> Result<T, dhall::error::Error> {
        let entries = self.builtins.clone();

        Ctxt::with_new_custom(entries, |cx| {
            let parsed = Parsed::parse_str(input)?;
            let name_env = self.build_name_env();
            let resolved = self.resolve_with_names(cx, parsed, &name_env)?;

            let (hir, env) = if self.builtins.is_empty() {
                let typed = resolved.typecheck(cx)?;
                (typed.as_hir().clone(), NzEnv::new(cx))
            } else {
                let ty_env = self.build_ty_env(cx)?;
                let typed = resolved.typecheck_with_env(&ty_env)?;
                (typed.as_hir().clone(), ty_env.to_nzenv())
            };

            let lazy = Lazy { hir, env, cx };
            Ok(f(&lazy))
        })
    }
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
