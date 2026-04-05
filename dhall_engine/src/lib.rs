//! Extensible evaluation engine for Dhall.
//!
//! Wraps the `dhall` crate (unmodified) to support custom builtin functions.
//!
//! # How it works
//!
//! A custom builtin has two halves:
//!
//! - A **Dhall lambda** defining input/output types and returning placeholder
//!   (sentinel) values. Dhall typechecks and normalizes this like any function.
//! - A **Rust `apply`** that computes the real output from normalized input.
//!
//! The engine prepends `let <name> = <dhall_lambda> in ...` to the user's
//! source, runs the standard Dhall pipeline, then walks the result replacing
//! sentinel records with real computed values. After rewriting, the NIR is
//! converted back to HIR and re-normalized so that expressions depending on
//! builtin output reduce correctly.
//!
//! # Sentinel convention
//!
//! The Dhall lambda must:
//! 1. Store the original input under key `__dhall_engine_input`.
//! 2. Include at least one text field starting with [`SENTINEL_PREFIX`]
//!    followed by the builtin name.
//!
//! ```dhall
//! \(input : { name : Text, src : Text }) ->
//!   { hash = "__dhall_engine_sentinel:myBuiltin"
//!   , __dhall_engine_input = input
//!   }
//! ```
//!
//! # Constraints
//!
//! - **`Resolved` is opaque.** We can't extract `Hir` from it, so we
//!   round-trip through `Expr` (re-parse + re-resolve) when injecting builtins.
//! - **No typecheck bypass.** The Dhall lambda must be valid Dhall so the
//!   standard typecheck passes.
//! - **Re-normalization.** After replacing sentinels with real values, the
//!   engine converts the NIR back to HIR and re-normalizes. This allows
//!   Dhall expressions that depend on builtin output to reduce correctly,
//!   BUT only when the sentinel record survives intact until the rewrite
//!   pass. If Dhall destructures the sentinel (e.g. `.result` field access),
//!   the sentinel is consumed during the first normalization and the rewriter
//!   cannot intercept it.

pub mod conv;
pub mod resolve;

use std::collections::HashMap;

use dhall::semantics::{Nir, NirKind};
use dhall::syntax::Label;
use dhall::{Ctxt, Parsed};

/// Re-export core dhall types needed by engine users.
pub mod types {
    pub use dhall::builtins::Builtin;
    pub use dhall::operations::OpKind;
    pub use dhall::semantics::{Nir, NirKind, TextLit};
    pub use dhall::syntax::{Expr, ExprKind, Label, NumKind, Span};
    pub use dhall::{Ctxt, Normalized, Parsed, Resolved, Typed};
    pub use crate::conv::{DhallType, FromNir, IntoNir, NirExt, NirRecord, NirRecordBuilder};
}

pub const SENTINEL_PREFIX: &str = "__dhall_engine_sentinel:";
const INPUT_KEY: &str = "__dhall_engine_input";

// ── Trait ────────────────────────────────────────────────────────────

/// A typed data transformer: Dhall lambda + Rust evaluation logic.
pub trait CustomBuiltin {
    /// Name used to reference this builtin in Dhall source.
    fn name(&self) -> &str;
    /// Complete Dhall lambda expression. See [crate docs](crate) for convention.
    fn dhall_expr(&self) -> &str;
    /// Compute real output from fully-normalized input. `None` = leave sentinel.
    fn apply<'cx>(&self, arg: Nir<'cx>, cx: Ctxt<'cx>) -> Option<Nir<'cx>>;
}

// ── Engine ───────────────────────────────────────────────────────────

pub struct Engine<R = resolve::DefaultResolver> {
    resolver: R,
    builtins: Vec<Box<dyn CustomBuiltin>>,
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

    pub fn with_builtin(mut self, b: impl CustomBuiltin + 'static) -> Self {
        self.builtins.push(Box::new(b));
        self
    }
}

impl<R: resolve::ImportResolver> Engine<R> {
    pub fn eval_str(&self, input: &str) -> Result<dhall::syntax::Expr, dhall::error::Error> {
        Ctxt::with_new(|cx| {
            let nir = self.eval_to_nir(cx, input)?;
            Ok(nir.to_expr(cx, Default::default()))
        })
    }

    fn eval_to_nir<'cx>(&self, cx: Ctxt<'cx>, input: &str) -> Result<Nir<'cx>, dhall::error::Error> {
        let parsed = Parsed::parse_str(input)?;

        if self.builtins.is_empty() {
            return self.run_pipeline(cx, parsed);
        }

        let mut src = String::new();
        for b in &self.builtins {
            use std::fmt::Write;
            let _ = write!(src, "let {} = {} in\n", b.name(), b.dhall_expr());
        }
        src.push_str(&parsed.to_expr().to_string());

        let nir = self.run_pipeline(cx, Parsed::parse_str(&src)?)?;

        let builtins: HashMap<&str, &dyn CustomBuiltin> =
            self.builtins.iter().map(|b| (b.name(), b.as_ref())).collect();
        let rewritten = rewrite(cx, &nir, &builtins);

        // Re-normalize: convert rewritten NIR → HIR → NIR.
        // This lets Dhall reduce expressions that now have real values
        // where sentinels used to be.
        let hir = rewritten.to_hir_noenv();
        Ok(hir.eval_closed_expr(cx))
    }

    fn run_pipeline<'cx>(&self, cx: Ctxt<'cx>, p: Parsed) -> Result<Nir<'cx>, dhall::error::Error> {
        Ok(self.resolver.resolve(cx, p)?.typecheck(cx)?.normalize(cx).as_nir().clone())
    }
}

// ── NIR rewriting ────────────────────────────────────────────────────

type BMap<'a> = HashMap<&'a str, &'a dyn CustomBuiltin>;

fn match_sentinel<'cx>(nir: &Nir<'cx>) -> Option<(String, Nir<'cx>)> {
    let fields = match nir.kind() { NirKind::RecordLit(f) => f, _ => return None };
    let input = fields.get(&Label::from(INPUT_KEY))?.clone();
    for v in fields.values() {
        if let NirKind::TextLit(txt) = v.kind() {
            if let Some(s) = txt.as_text() {
                if let Some(rest) = s.strip_prefix(SENTINEL_PREFIX) {
                    return Some((rest.split(':').next().unwrap_or(rest).to_owned(), input));
                }
            }
        }
    }
    None
}

fn rewrite<'cx>(cx: Ctxt<'cx>, nir: &Nir<'cx>, b: &BMap<'_>) -> Nir<'cx> {
    if let Some((name, input)) = match_sentinel(nir) {
        if let Some(builtin) = b.get(name.as_str()) {
            let input = rewrite(cx, &input, b);
            if let Some(result) = builtin.apply(input, cx) {
                return rewrite(cx, &result, b);
            }
        }
    }
    rewrite_children(cx, nir, b)
}

fn rewrite_children<'cx>(cx: Ctxt<'cx>, nir: &Nir<'cx>, b: &BMap<'_>) -> Nir<'cx> {
    use dhall::syntax::InterpolatedTextContents as ITC;
    let rw = |n: &Nir<'cx>| rewrite(cx, n, b);
    let rw_map = |f: &HashMap<Label, Nir<'cx>>| f.iter().map(|(k, v)| (k.clone(), rw(v))).collect();
    let rw_opt_map = |f: &HashMap<Label, Option<Nir<'cx>>>| {
        f.iter().map(|(k, v)| (k.clone(), v.as_ref().map(|v| rw(v)))).collect()
    };

    let kind = match nir.kind() {
        NirKind::RecordLit(f)  => NirKind::RecordLit(rw_map(f)),
        NirKind::RecordType(f) => NirKind::RecordType(rw_map(f)),
        NirKind::NEListLit(es) => NirKind::NEListLit(es.iter().map(rw).collect()),
        NirKind::UnionType(k)  => NirKind::UnionType(rw_opt_map(k)),
        NirKind::UnionConstructor(l, k) => NirKind::UnionConstructor(l.clone(), rw_opt_map(k)),
        NirKind::UnionLit(l, v, k)      => NirKind::UnionLit(l.clone(), rw(v), rw_opt_map(k)),
        NirKind::Equivalence(l, r) => NirKind::Equivalence(rw(l), rw(r)),
        NirKind::Op(op) => NirKind::Op(op.map_ref(rw)),
        NirKind::TextLit(t) => NirKind::TextLit(dhall::semantics::TextLit::new(
            t.iter().map(|c| match c { ITC::Text(s) => ITC::Text(s.clone()), ITC::Expr(e) => ITC::Expr(rw(e)) })
        )),
        NirKind::EmptyListLit(t)    => NirKind::EmptyListLit(rw(t)),
        NirKind::ListType(t)        => NirKind::ListType(rw(t)),
        NirKind::OptionalType(t)    => NirKind::OptionalType(rw(t)),
        NirKind::EmptyOptionalLit(t)=> NirKind::EmptyOptionalLit(rw(t)),
        NirKind::NEOptionalLit(v)   => NirKind::NEOptionalLit(rw(v)),
        NirKind::Assert(t)          => NirKind::Assert(rw(t)),
        _ => return nir.clone(),
    };
    Nir::from_kind(kind)
}
