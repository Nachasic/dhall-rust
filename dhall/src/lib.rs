#![cfg_attr(not(feature = "std"), no_std)]
#![doc(html_root_url = "https://docs.rs/dhall/0.13.0")]
#![allow(
    clippy::implicit_hasher,
    clippy::module_inception,
    clippy::needless_lifetimes,
    clippy::needless_question_mark,
    clippy::new_ret_no_self,
    clippy::new_without_default,
    clippy::try_err,
    clippy::unnecessary_wraps,
    clippy::upper_case_acronyms,
    clippy::useless_format,
    unknown_lints
)]

extern crate alloc;

pub mod builtins;
pub mod ctxt;
pub mod error;
pub mod operations;
pub mod semantics;
pub mod syntax;
pub mod utils;

#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
use std::path::Path;
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
use url::Url;

use alloc::boxed::Box;

use crate::error::{Error, TypeError};
use crate::semantics::parse;
use crate::semantics::resolve;
use crate::semantics::resolve::ImportLocation;
use crate::semantics::{typecheck, typecheck_with, Hir, Nir, Tir, Type};
use crate::syntax::Expr;

pub use ctxt::*;

#[derive(Debug, Clone)]
pub struct Parsed(Expr, ImportLocation);

/// An expression where all imports have been resolved
///
/// Invariant: there must be no `Import` nodes or `ImportAlt` operations left.
#[derive(Debug, Clone)]
pub struct Resolved<'cx>(Hir<'cx>);

/// A typed expression
#[derive(Debug, Clone)]
pub struct Typed<'cx> {
    pub hir: Hir<'cx>,
    pub ty: Type<'cx>,
}

/// A normalized expression.
///
/// This is actually a lie, because the expression will only get normalized on demand.
#[derive(Debug, Clone)]
pub struct Normalized<'cx>(Nir<'cx>);

/// Controls conversion from `Nir` to `Expr`
#[derive(Copy, Clone, Default)]
pub struct ToExprOptions {
    /// Whether to convert all variables to `_`
    pub alpha: bool,
}

impl Parsed {
    /// Construct from an `Expr`. This `Expr` will have imports disabled.
    pub fn from_expr_without_imports(e: Expr) -> Self {
        Parsed(e, ImportLocation::dhall_code_without_imports())
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
    pub fn parse_file(f: &Path) -> Result<Parsed, Error> {
        parse::parse_file(f)
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
    pub fn parse_remote(url: Url) -> Result<Parsed, Error> {
        parse::parse_remote(url)
    }
    pub fn parse_str(s: &str) -> Result<Parsed, Error> {
        parse::parse_str(s)
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
    pub fn parse_binary_file(f: &Path) -> Result<Parsed, Error> {
        parse::parse_binary_file(f)
    }
    #[allow(dead_code)]
    pub fn parse_binary(data: &[u8]) -> Result<Parsed, Error> {
        parse::parse_binary(data)
    }

    pub fn resolve<'cx>(self, cx: Ctxt<'cx>) -> Result<Resolved<'cx>, Error> {
        resolve::resolve(cx, self)
    }

    pub fn resolve_with_fetcher<'cx>(
        self,
        cx: Ctxt<'cx>,
        fetcher: Box<dyn semantics::ImportFetcher>,
    ) -> Result<Resolved<'cx>, Error> {
        resolve::resolve_with_fetcher(cx, self, fetcher)
    }

    /// Resolve with extra names pre-populated in the environment.
    /// Custom builtin names should be inserted into `names` so the
    /// resulting `Hir` resolves them as variables instead of `MissingVar`.
    pub fn resolve_with_names<'cx>(
        self,
        cx: Ctxt<'cx>,
        names: &semantics::NameEnv,
    ) -> Result<Resolved<'cx>, Error> {
        resolve::resolve_with_names(cx, self, names)
    }

    /// Like `resolve_with_names`, but also uses a custom import fetcher.
    pub fn resolve_with_names_and_fetcher<'cx>(
        self,
        cx: Ctxt<'cx>,
        names: &semantics::NameEnv,
        fetcher: Box<dyn semantics::ImportFetcher>,
    ) -> Result<Resolved<'cx>, Error> {
        resolve::resolve_with_names_and_fetcher(cx, self, names, fetcher)
    }

    pub fn skip_resolve<'cx>(
        self,
        cx: Ctxt<'cx>,
    ) -> Result<Resolved<'cx>, Error> {
        resolve::skip_resolve(cx, self)
    }

    /// Converts a value back to the corresponding AST expression.
    pub fn to_expr(&self) -> Expr {
        self.0.clone()
    }

    pub fn add_let_binding(self, label: syntax::Label, value: Expr) -> Parsed {
        let Parsed(expr, import_location) = self;
        Parsed(expr.add_let_binding(label, value), import_location)
    }
}

/// Convert an `Expr` to a `Hir`, resolving variables against the given `NameEnv`.
/// Variables not found in the environment become `MissingVar` (which will
/// panic during normalization — ensure all free variables are accounted for).
/// This does not handle imports; use on import-free expressions only.
pub fn expr_to_hir<'cx>(
    expr: &Expr,
    env: &mut semantics::NameEnv,
) -> semantics::Hir<'cx> {
    use semantics::{Hir, HirKind};

    let kind = match expr.kind() {
        syntax::ExprKind::Var(v) => match env.unlabel_var(v) {
            Some(alpha) => HirKind::Var(alpha),
            None => HirKind::MissingVar(v.clone()),
        },
        syntax::ExprKind::Builtin(b) => HirKind::Expr(syntax::ExprKind::Builtin(*b)),
        other => {
            let mapped = other.map_ref_maybe_binder(|binder, sub| {
                if let Some(label) = binder { env.insert_mut(label); }
                let hir = expr_to_hir(sub, env);
                if binder.is_some() { env.remove_mut(); }
                hir
            });
            HirKind::Expr(mapped)
        }
    };
    Hir::new(kind, expr.span())
}

impl<'cx> Resolved<'cx> {
    pub fn typecheck(&self, cx: Ctxt<'cx>) -> Result<Typed<'cx>, TypeError> {
        Ok(Typed::from_tir(typecheck(cx, &self.0)?))
    }
    pub fn typecheck_with(
        self,
        cx: Ctxt<'cx>,
        ty: &Hir<'cx>,
    ) -> Result<Typed<'cx>, TypeError> {
        Ok(Typed::from_tir(typecheck_with(cx, &self.0, ty)?))
    }
    /// Typecheck against a pre-populated type environment.
    /// Use this when custom bindings (e.g. custom builtins) have been
    /// injected during resolution via `resolve_with_names`.
    pub fn typecheck_with_env(
        &self,
        ty_env: &semantics::TyEnv<'cx>,
    ) -> Result<Typed<'cx>, TypeError> {
        Ok(Typed::from_tir(semantics::type_with(ty_env, &self.0, None)?))
    }
    /// Converts a value back to the corresponding AST expression.
    pub fn to_expr(&self, cx: Ctxt<'cx>) -> Expr {
        self.0.to_expr_noopts(cx)
    }
    /// Access the inner `Hir` for direct traversal or lazy evaluation.
    pub fn as_hir(&self) -> &Hir<'cx> {
        &self.0
    }
}

impl<'cx> Typed<'cx> {
    fn from_tir(tir: Tir<'cx, '_>) -> Self {
        Typed {
            hir: tir.as_hir().clone(),
            ty: tir.ty().clone(),
        }
    }
    /// Reduce an expression to its normal form, performing beta reduction
    pub fn normalize(&self, cx: Ctxt<'cx>) -> Normalized<'cx> {
        Normalized(self.hir.eval_closed_expr(cx))
    }

    /// Evaluate the expression in the given environment, returning the normalized value.
    /// Use this when typechecking was done with a custom `TyEnv` (via `typecheck_with_env`).
    pub fn eval_to_nir(&self, env: &semantics::NzEnv<'cx>) -> Nir<'cx> {
        self.hir.eval(env)
    }

    /// Converts a value back to the corresponding AST expression.
    fn to_expr(&self, cx: Ctxt<'cx>) -> Expr {
        self.hir.to_expr(cx, ToExprOptions { alpha: false })
    }

    pub fn as_hir(&self) -> &Hir<'cx> {
        &self.hir
    }
    pub fn ty(&self) -> &Type<'cx> {
        &self.ty
    }
    pub fn get_type(&self) -> Result<Normalized<'cx>, TypeError> {
        Ok(Normalized(self.ty.clone().into_nir()))
    }
}

impl<'cx> Normalized<'cx> {
    /// Converts a value back to the corresponding AST expression.
    pub fn to_expr(&self, cx: Ctxt<'cx>) -> Expr {
        self.0.to_expr(cx, ToExprOptions::default())
    }
    /// Converts a value back to the corresponding Hir expression.
    pub fn to_hir(&self) -> Hir<'cx> {
        self.0.to_hir_noenv()
    }
    pub fn as_nir(&self) -> &Nir<'cx> {
        &self.0
    }
    /// Converts a value back to the corresponding AST expression, alpha-normalizing in the process.
    pub fn to_expr_alpha(&self, cx: Ctxt<'cx>) -> Expr {
        self.0.to_expr(cx, ToExprOptions { alpha: true })
    }
}

macro_rules! derive_traits_for_wrapper_struct {
    ($ty:ident) => {
        impl core::cmp::PartialEq for $ty {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }

        impl core::cmp::Eq for $ty {}

        impl core::fmt::Display for $ty {
            fn fmt(
                &self,
                f: &mut core::fmt::Formatter,
            ) -> Result<(), core::fmt::Error> {
                self.0.fmt(f)
            }
        }
    };
}

derive_traits_for_wrapper_struct!(Parsed);

impl From<Parsed> for Expr {
    fn from(other: Parsed) -> Self {
        other.to_expr()
    }
}

impl<'cx> Eq for Normalized<'cx> {}
impl<'cx> PartialEq for Normalized<'cx> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
