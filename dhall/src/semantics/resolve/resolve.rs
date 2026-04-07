use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::builtins::Builtin;
use crate::error::ErrorBuilder;
use crate::error::Error;
use crate::operations::{BinOp, OpKind};
use crate::semantics::{mkerr, Hir, HirKind, ImportEnv, ImportFetcher, NameEnv, NoImports, Type};
use crate::syntax;
use crate::syntax::{
    Expr, ExprKind, FilePath, ImportMode, ImportTarget, Span,
    UnspannedExpr, URL,
};
use crate::{
    Ctxt, ImportAlternativeId, ImportId, ImportResultId, Parsed, Resolved,
    Typed,
};
#[cfg(not(target_arch = "wasm32"))]
use url::Url;

// TODO: evaluate import headers
pub type Import = syntax::Import<()>;

/// Path representation: `PathBuf` when `std` is available, `String` otherwise.
#[cfg(feature = "std")]
pub type LocalPath = std::path::PathBuf;
#[cfg(not(feature = "std"))]
pub type LocalPath = String;

/// The location of some data, usually some dhall code.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImportLocationKind {
    /// Local file
    Local(LocalPath),
    /// Remote file
    #[cfg(not(target_arch = "wasm32"))]
    Remote(Url),
    /// Environment variable
    Env(String),
    /// Data without a location; chaining will start from current directory.
    Missing,
    /// Token to signal that this file should contain no imports.
    NoImport,
}

/// The location of some data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImportLocation {
    kind: ImportLocationKind,
    mode: ImportMode,
}

impl ImportLocation {
    pub fn kind(&self) -> &ImportLocationKind { &self.kind }
    pub fn mode(&self) -> ImportMode { self.mode }

    /// Create an import location from a kind and mode.
    pub fn new(kind: ImportLocationKind, mode: ImportMode) -> Self {
        ImportLocation { kind, mode }
    }

    /// Create a local file import location.
    pub fn local(path: LocalPath, mode: ImportMode) -> Self {
        ImportLocation { kind: ImportLocationKind::Local(path), mode }
    }

    pub fn dhall_code_of_unknown_origin() -> Self {
        ImportLocation {
            kind: ImportLocationKind::Missing,
            mode: ImportMode::Code,
        }
    }
    pub fn dhall_code_without_imports() -> Self {
        ImportLocation {
            kind: ImportLocationKind::NoImport,
            mode: ImportMode::Code,
        }
    }
    pub fn local_dhall_code(path: LocalPath) -> Self {
        ImportLocation {
            kind: ImportLocationKind::Local(path),
            mode: ImportMode::Code,
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn remote_dhall_code(url: Url) -> Self {
        ImportLocation {
            kind: ImportLocationKind::Remote(url),
            mode: ImportMode::Code,
        }
    }
}

impl ImportLocationKind {
    pub fn to_location(&self) -> Expr {
        let (field_name, arg) = match self {
            ImportLocationKind::Local(path) => {
                #[cfg(feature = "std")]
                let s = path.to_string_lossy().into_owned();
                #[cfg(not(feature = "std"))]
                let s = path.clone();
                ("Local", Some(s))
            }
            #[cfg(not(target_arch = "wasm32"))]
            ImportLocationKind::Remote(url) => {
                ("Remote", Some(url.to_string()))
            }
            ImportLocationKind::Env(name) => {
                ("Environment", Some(name.clone()))
            }
            ImportLocationKind::Missing => ("Missing", None),
            ImportLocationKind::NoImport => unreachable!(),
        };

        let asloc_ty = make_aslocation_uniontype();
        let expr =
            mkexpr(ExprKind::Op(OpKind::Field(asloc_ty, field_name.into())));
        match arg {
            Some(arg) => mkexpr(ExprKind::Op(OpKind::App(
                expr,
                mkexpr(ExprKind::TextLit(arg.into())),
            ))),
            None => expr,
        }
    }
}

fn mkexpr(kind: UnspannedExpr) -> Expr {
    Expr::new(kind, Span::Artificial)
}

fn make_aslocation_uniontype() -> Expr {
    let text_type = mkexpr(ExprKind::Builtin(Builtin::Text));
    let mut union = BTreeMap::default();
    union.insert("Local".into(), Some(text_type.clone()));
    union.insert("Remote".into(), Some(text_type.clone()));
    union.insert("Environment".into(), Some(text_type));
    union.insert("Missing".into(), None);
    mkexpr(ExprKind::UnionType(union))
}

pub fn check_hash<'cx>(
    cx: Ctxt<'cx>,
    import: ImportId<'cx>,
    result: ImportResultId<'cx>,
) -> Result<(), Error> {
    let import = &cx[import];
    if let (ImportMode::Code, Some(syntax::Hash::SHA256(hash))) =
        (import.import.mode, &import.import.hash)
    {
        let expr = cx[result].hir.to_expr_alpha(cx);
        let actual_hash = expr.sha256_hash()?;
        if hash[..] != actual_hash[..] {
            mkerr(
                ErrorBuilder::new("hash mismatch")
                    .span_err(import.span.clone(), "hash mismatch")
                    .note(format!("Expected sha256:{}", hex::encode(hash)))
                    .note(format!(
                        "Found    sha256:{}",
                        hex::encode(actual_hash)
                    ))
                    .format(),
            )?
        }
    }
    Ok(())
}

/// Desugar the first level of the expression.
fn desugar(expr: &Expr) -> Cow<'_, Expr> {
    match expr.kind() {
        ExprKind::Op(OpKind::Completion(ty, compl)) => {
            let ty_field_default = Expr::new(
                ExprKind::Op(OpKind::Field(ty.clone(), "default".into())),
                expr.span(),
            );
            let merged = Expr::new(
                ExprKind::Op(OpKind::BinOp(
                    BinOp::RightBiasedRecordMerge,
                    ty_field_default,
                    compl.clone(),
                )),
                expr.span(),
            );
            let ty_field_type = Expr::new(
                ExprKind::Op(OpKind::Field(ty.clone(), "Type".into())),
                expr.span(),
            );
            Cow::Owned(Expr::new(
                ExprKind::Annot(merged, ty_field_type),
                expr.span(),
            ))
        }
        _ => Cow::Borrowed(expr),
    }
}

/// Fetch the import and store the result in the global context.
fn fetch_import<'cx>(
    env: &mut ImportEnv<'cx>,
    import_id: ImportId<'cx>,
) -> Result<ImportResultId<'cx>, Error> {
    let cx = env.cx();
    let import = &cx[import_id].import;
    let span = cx[import_id].span.clone();

    let location = env.fetcher().chain(&cx[import_id].base_location, import)?;

    // If the hash is in the on-disk cache, return the cached contents.
    if let Some(typed) = env.get_from_disk_cache(&import.hash) {
        let res_id = cx.push_import_result(typed);
        return Ok(res_id);
    }

    // If the import is in the in-memory cache return the cached contents.
    // Otherwise fetch the import.
    let res_id = if let Some(res_id) = env.get_from_mem_cache(&location) {
        res_id
    } else {
        let res = env.with_cycle_detection(location.clone(), |env| {
            let typed = match location.mode {
                ImportMode::Location => {
                    let expr = location.kind.to_location();
                    Parsed::from_expr_without_imports(expr)
                        .skip_resolve(cx)?
                        .typecheck(cx)?
                }
                _ => {
                    let source = env.fetcher().fetch(&location)?;
                    match location.mode {
                        ImportMode::Code => {
                            let expr = crate::syntax::parse_expr(&source)?;
                            let parsed = Parsed(expr, location.clone());
                            let typed = resolve_with_env(env, parsed)?.typecheck(cx)?;
                            Typed {
                                hir: typed.normalize(cx).to_hir(),
                                ty: typed.ty,
                            }
                        }
                        ImportMode::RawText => Typed {
                            hir: Hir::new(
                                HirKind::Expr(ExprKind::TextLit(source.into())),
                                span.clone(),
                            ),
                            ty: Type::from_builtin(cx, Builtin::Text),
                        },
                        ImportMode::Location => unreachable!(),
                    }
                }
            };
            Ok(typed)
        });
        let typed = match res {
            Ok(typed) => typed,
            Err(e) => mkerr(
                ErrorBuilder::new("error")
                    .span_err(span.clone(), e.to_string())
                    .format(),
            )?,
        };

        let res_id = cx.push_import_result(typed);
        env.write_to_mem_cache(location, res_id);
        res_id
    };

    // Add the resolved import to the on-disk cache if the hash matches.
    env.check_hash(import_id, res_id)?;
    env.write_to_disk_cache(&import.hash, res_id);

    Ok(res_id)
}

/// Part of a tree of imports.
#[derive(Debug, Clone, Copy)]
pub enum ImportNode<'cx> {
    Import(ImportId<'cx>),
    Alternative(ImportAlternativeId<'cx>),
}

/// Traverse the expression and replace each import and import alternative by an id into the global
/// context. The ids are also accumulated into `nodes` so that we can resolve them afterwards.
fn traverse_accumulate<'cx>(
    env: &mut ImportEnv<'cx>,
    name_env: &mut NameEnv,
    nodes: &mut Vec<ImportNode<'cx>>,
    base_location: &ImportLocation,
    expr: &Expr,
) -> Hir<'cx> {
    let cx = env.cx();
    let expr = desugar(expr);
    let kind = match expr.kind() {
        ExprKind::Var(var) => match name_env.unlabel_var(&var) {
            Some(v) => HirKind::Var(v),
            None => HirKind::MissingVar(var.clone()),
        },
        ExprKind::Op(OpKind::BinOp(BinOp::ImportAlt, l, r)) => {
            let mut imports_l = Vec::new();
            let l = traverse_accumulate(
                env,
                name_env,
                &mut imports_l,
                base_location,
                l,
            );
            let mut imports_r = Vec::new();
            let r = traverse_accumulate(
                env,
                name_env,
                &mut imports_r,
                base_location,
                r,
            );
            let alt =
                cx.push_import_alternative(imports_l.into(), imports_r.into());
            nodes.push(ImportNode::Alternative(alt));
            HirKind::ImportAlternative(alt, l, r)
        }
        kind => {
            let kind = kind.map_ref_maybe_binder(|l, e| {
                if let Some(l) = l {
                    name_env.insert_mut(l);
                }
                let hir =
                    traverse_accumulate(env, name_env, nodes, base_location, e);
                if l.is_some() {
                    name_env.remove_mut();
                }
                hir
            });
            match kind {
                ExprKind::Import(import) => {
                    // TODO: evaluate import headers
                    let import = import.map_ref(|_| ());
                    let import_id = cx.push_import(
                        base_location.clone(),
                        import,
                        expr.span(),
                    );
                    nodes.push(ImportNode::Import(import_id));
                    HirKind::Import(import_id)
                }
                kind => HirKind::Expr(kind),
            }
        }
    };
    Hir::new(kind, expr.span())
}

/// Take a list of nodes and recursively resolve them.
fn resolve_nodes<'cx>(
    env: &mut ImportEnv<'cx>,
    nodes: &[ImportNode<'cx>],
) -> Result<(), Error> {
    for &node in nodes {
        match node {
            ImportNode::Import(import) => {
                let res_id = fetch_import(env, import)?;
                env.cx()[import].set_resultid(res_id);
            }
            ImportNode::Alternative(alt) => {
                let alt = &env.cx()[alt];
                if resolve_nodes(env, &alt.left_imports).is_ok() {
                    alt.set_selected(true);
                } else {
                    resolve_nodes(env, &alt.right_imports)?;
                    alt.set_selected(false);
                }
            }
        }
    }
    Ok(())
}

fn resolve_with_env<'cx>(
    env: &mut ImportEnv<'cx>,
    parsed: Parsed,
) -> Result<Resolved<'cx>, Error> {
    resolve_with_env_and_names(env, parsed, &NameEnv::new())
}

/// Like `resolve_with_env`, but starts with extra names already in scope.
pub fn resolve_with_env_and_names<'cx>(
    env: &mut ImportEnv<'cx>,
    parsed: Parsed,
    extra_names: &NameEnv,
) -> Result<Resolved<'cx>, Error> {
    let Parsed(expr, base_location) = parsed;
    let mut nodes = Vec::new();
    let mut name_env = extra_names.clone();
    let resolved = traverse_accumulate(
        env,
        &mut name_env,
        &mut nodes,
        &base_location,
        &expr,
    );
    resolve_nodes(env, &nodes)?;
    Ok(Resolved(resolved))
}

/// Resolve using a custom fetcher.
pub fn resolve_with_fetcher<'cx>(
    cx: Ctxt<'cx>,
    parsed: Parsed,
    fetcher: Box<dyn ImportFetcher>,
) -> Result<Resolved<'cx>, Error> {
    parsed.resolve_with_env(&mut ImportEnv::new(cx, fetcher))
}

/// Resolve with extra names and a custom fetcher.
pub fn resolve_with_names_and_fetcher<'cx>(
    cx: Ctxt<'cx>,
    parsed: Parsed,
    names: &NameEnv,
    fetcher: Box<dyn ImportFetcher>,
) -> Result<Resolved<'cx>, Error> {
    resolve_with_env_and_names(&mut ImportEnv::new(cx, fetcher), parsed, names)
}

/// Resolves names, and errors if we find any imports.
pub fn skip_resolve<'cx>(
    cx: Ctxt<'cx>,
    parsed: Parsed,
) -> Result<Resolved<'cx>, Error> {
    let parsed = Parsed::from_expr_without_imports(parsed.0);
    resolve_with_fetcher(cx, parsed, Box::new(NoImports))
}

impl Parsed {
    fn resolve_with_env<'cx>(
        self,
        env: &mut ImportEnv<'cx>,
    ) -> Result<Resolved<'cx>, Error> {
        resolve_with_env(env, self)
    }
}

pub trait Canonicalize {
    fn canonicalize(&self) -> Self;
}

impl Canonicalize for FilePath {
    fn canonicalize(&self) -> FilePath {
        let mut file_path = Vec::new();

        for c in &self.file_path {
            match c.as_ref() {
                "." => continue,
                ".." => match file_path.last() {
                    None => file_path.push("..".to_string()),
                    Some(c) if c == ".." => file_path.push("..".to_string()),
                    Some(_) => {
                        file_path.pop();
                    }
                },
                _ => file_path.push(c.clone()),
            }
        }

        FilePath { file_path }
    }
}

impl<SE: Copy> Canonicalize for ImportTarget<SE> {
    fn canonicalize(&self) -> ImportTarget<SE> {
        match self {
            ImportTarget::Local(prefix, file) => {
                ImportTarget::Local(*prefix, file.canonicalize())
            }
            ImportTarget::Remote(url) => ImportTarget::Remote(URL {
                scheme: url.scheme,
                authority: url.authority.clone(),
                path: url.path.canonicalize(),
                query: url.query.clone(),
                headers: url.headers,
            }),
            ImportTarget::Env(name) => ImportTarget::Env(name.to_string()),
            ImportTarget::Missing => ImportTarget::Missing,
        }
    }
}
