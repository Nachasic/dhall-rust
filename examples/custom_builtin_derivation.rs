// ⚠️  WARNING: This file is a CONCEPTUAL EXAMPLE ONLY
// ⚠️  It does NOT compile and is NOT meant to be run
// ⚠️  It demonstrates what custom builtins would look like if you forked dhall-rust
// ⚠️  
// ⚠️  For WORKING examples, see:
// ⚠️  - derivation_system.rs (minimal, no Dhall)
// ⚠️  - dhall_derivation_integration.rs (full Dhall integration)

use dhall::builtins::{Builtin, BuiltinClosure};
use dhall::semantics::{Hir, HirKind, Nir, NirKind, NzEnv};
use dhall::syntax::{Expr, ExprKind, Label, NumKind, Span};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type StoreHash = String;

#[derive(Clone)]
struct Store {
    data: Arc<Mutex<HashMap<StoreHash, u64>>>,
}

impl Store {
    fn new() -> Self {
        Store {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn put(&self, value: u64) -> StoreHash {
        let hash = format!("sha256:{:016x}", blake3::hash(&value.to_le_bytes()));
        self.data.lock().unwrap().insert(hash.clone(), value);
        hash
    }

    fn get(&self, hash: &str) -> Option<u64> {
        self.data.lock().unwrap().get(hash).copied()
    }
}


/// Custom builtin that stores a number and returns its hash
fn apply_mk_derivation<'cx>(
    args: &[Nir<'cx>],
    store: &Store,
) -> Option<NirKind<'cx>> {
    if args.len() != 1 {
        return None; // Need exactly 1 argument
    }

    let config = &args[0];

    // Extract the record { type : Text, value : Natural }
    let fields = match config.kind() {
        NirKind::RecordLit(f) => f,
        _ => return None,
    };

    // Extract type field
    let type_field = fields.get(&Label::from("type"))?;
    let type_text = match type_field.kind() {
        NirKind::TextLit(txt) => txt.as_text()?,
        _ => return None,
    };

    // Only support "number" type
    if type_text != "number" {
        return None;
    }

    // Extract value field
    let value_field = fields.get(&Label::from("value"))?;
    let value = match value_field.kind() {
        NirKind::Num(NumKind::Natural(n)) => *n,
        _ => return None,
    };

    // Store the value and get hash
    let hash = store.put(value);

    // Return { type : Text, hash : Text }
    let mut result = HashMap::new();
    result.insert(
        Label::from("type"),
        Nir::from_text(type_text),
    );
    result.insert(
        Label::from("hash"),
        Nir::from_text(hash),
    );

    Some(NirKind::RecordLit(result))
}


/// Parse and evaluate Dhall with custom builtin support
fn eval_with_custom_builtin(
    dhall_code: &str,
    store: &Store,
) -> Result<Nir<'static>, String> {
    use dhall::Ctxt;

    let cx = Ctxt::new();

    // Parse
    let parsed = dhall::syntax::parse_expr(dhall_code)
        .map_err(|e| format!("Parse error: {:?}", e))?;

    // Resolve (handle imports)
    let resolved = resolve_with_custom_builtin(cx, parsed, store)?;

    // Typecheck
    let typed = resolved
        .typecheck(cx)
        .map_err(|e| format!("Type error: {:?}", e))?;

    // Normalize
    Ok(typed.normalize())
}

/// Custom resolver that intercepts Runtime/mkDerivation
fn resolve_with_custom_builtin<'cx>(
    cx: Ctxt<'cx>,
    expr: Expr,
    store: &Store,
) -> Result<Hir<'cx>, String> {
    let hir = resolve_expr(cx, &expr, store);
    Ok(hir)
}

fn resolve_expr<'cx>(cx: Ctxt<'cx>, expr: &Expr, store: &Store) -> Hir<'cx> {
    let kind = match expr.kind() {
        ExprKind::Var(v) => {
            // Check if it's our custom builtin
            if v.0.as_ref() == "Runtime/mkDerivation" && v.1 == 0 {
                // Return a special marker that we'll handle during evaluation
                // For simplicity, we'll use a builtin placeholder
                HirKind::Expr(ExprKind::Builtin(Builtin::NaturalShow))
            } else {
                HirKind::MissingVar(v.clone())
            }
        }
        ExprKind::Lam(binder, annot, body) => {
            let annot = resolve_expr(cx, annot, store);
            let body = resolve_expr(cx, body, store);
            HirKind::Expr(ExprKind::Lam(binder.clone(), annot, body))
        }
        ExprKind::App(f, a) => {
            let f = resolve_expr(cx, f, store);
            let a = resolve_expr(cx, a, store);

            // Check if we're applying Runtime/mkDerivation
            if is_mk_derivation(&f) {
                // Evaluate the argument and apply our custom logic
                let arg_nir = a.eval(NzEnv::new(cx));
                if let Some(result) = apply_mk_derivation(&[arg_nir], store) {
                    return Hir::new(
                        HirKind::Expr(nir_to_expr(result)),
                        Span::Artificial,
                    );
                }
            }

            HirKind::Expr(ExprKind::App(f, a))
        }
        ExprKind::RecordLit(kvs) => {
            let kvs = kvs
                .iter()
                .map(|(k, v)| (k.clone(), resolve_expr(cx, v, store)))
                .collect();
            HirKind::Expr(ExprKind::RecordLit(kvs))
        }
        ExprKind::RecordType(kvs) => {
            let kvs = kvs
                .iter()
                .map(|(k, v)| (k.clone(), resolve_expr(cx, v, store)))
                .collect();
            HirKind::Expr(ExprKind::RecordType(kvs))
        }
        other => HirKind::Expr(other.traverse_ref(|e| Ok(resolve_expr(cx, e, store))).unwrap()),
    };

    Hir::new(kind, expr.span())
}

fn is_mk_derivation(hir: &Hir) -> bool {
    matches!(
        hir.kind(),
        HirKind::Expr(ExprKind::Builtin(Builtin::NaturalShow))
    )
}

fn nir_to_expr(nir: NirKind) -> ExprKind<Hir<'static>> {
    match nir {
        NirKind::RecordLit(kvs) => ExprKind::RecordLit(
            kvs.into_iter()
                .map(|(k, v)| {
                    let expr = match v.kind() {
                        NirKind::TextLit(txt) => {
                            ExprKind::TextLit(txt.as_text().unwrap().into())
                        }
                        _ => ExprKind::TextLit("".into()),
                    };
                    (k, Hir::new(HirKind::Expr(expr), Span::Artificial))
                })
                .collect(),
        ),
        _ => ExprKind::TextLit("".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mk_derivation_basic() {
        let store = Store::new();

        let dhall_code = r#"
            let mkDerivation = Runtime/mkDerivation
            in mkDerivation { type = "number", value = 42 }
        "#;

        let result = eval_with_custom_builtin(dhall_code, &store).unwrap();

        // Check result is a record with type and hash
        let fields = match result.kind() {
            NirKind::RecordLit(f) => f,
            _ => panic!("Expected record, got {:?}", result),
        };

        // Check type field
        let type_field = fields.get(&Label::from("type")).unwrap();
        let type_text = match type_field.kind() {
            NirKind::TextLit(txt) => txt.as_text().unwrap(),
            _ => panic!("Expected text"),
        };
        assert_eq!(type_text, "number");

        // Check hash field exists
        let hash_field = fields.get(&Label::from("hash")).unwrap();
        let hash = match hash_field.kind() {
            NirKind::TextLit(txt) => txt.as_text().unwrap(),
            _ => panic!("Expected text"),
        };
        assert!(hash.starts_with("sha256:"));

        // Verify value is in store
        let stored_value = store.get(&hash).unwrap();
        assert_eq!(stored_value, 42);
    }

    #[test]
    fn test_mk_derivation_multiple_values() {
        let store = Store::new();

        let dhall_code = r#"
            let mkDerivation = Runtime/mkDerivation
            let drv1 = mkDerivation { type = "number", value = 100 }
            let drv2 = mkDerivation { type = "number", value = 200 }
            in { first = drv1, second = drv2 }
        "#;

        let result = eval_with_custom_builtin(dhall_code, &store).unwrap();

        let fields = match result.kind() {
            NirKind::RecordLit(f) => f,
            _ => panic!("Expected record"),
        };

        // Check first derivation
        let first = fields.get(&Label::from("first")).unwrap();
        let first_fields = match first.kind() {
            NirKind::RecordLit(f) => f,
            _ => panic!("Expected record"),
        };
        let first_hash = match first_fields.get(&Label::from("hash")).unwrap().kind() {
            NirKind::TextLit(txt) => txt.as_text().unwrap(),
            _ => panic!("Expected text"),
        };

        // Check second derivation
        let second = fields.get(&Label::from("second")).unwrap();
        let second_fields = match second.kind() {
            NirKind::RecordLit(f) => f,
            _ => panic!("Expected record"),
        };
        let second_hash = match second_fields.get(&Label::from("hash")).unwrap().kind() {
            NirKind::TextLit(txt) => txt.as_text().unwrap(),
            _ => panic!("Expected text"),
        };

        // Hashes should be different
        assert_ne!(first_hash, second_hash);

        // Both values should be in store
        assert_eq!(store.get(&first_hash).unwrap(), 100);
        assert_eq!(store.get(&second_hash).unwrap(), 200);
    }

    #[test]
    fn test_mk_derivation_same_value_same_hash() {
        let store = Store::new();

        let dhall_code = r#"
            let mkDerivation = Runtime/mkDerivation
            let drv1 = mkDerivation { type = "number", value = 42 }
            let drv2 = mkDerivation { type = "number", value = 42 }
            in { first = drv1, second = drv2 }
        "#;

        let result = eval_with_custom_builtin(dhall_code, &store).unwrap();

        let fields = match result.kind() {
            NirKind::RecordLit(f) => f,
            _ => panic!("Expected record"),
        };

        let first = fields.get(&Label::from("first")).unwrap();
        let first_fields = match first.kind() {
            NirKind::RecordLit(f) => f,
            _ => panic!("Expected record"),
        };
        let first_hash = match first_fields.get(&Label::from("hash")).unwrap().kind() {
            NirKind::TextLit(txt) => txt.as_text().unwrap(),
            _ => panic!("Expected text"),
        };

        let second = fields.get(&Label::from("second")).unwrap();
        let second_fields = match second.kind() {
            NirKind::RecordLit(f) => f,
            _ => panic!("Expected record"),
        };
        let second_hash = match second_fields.get(&Label::from("hash")).unwrap().kind() {
            NirKind::TextLit(txt) => txt.as_text().unwrap(),
            _ => panic!("Expected text"),
        };

        // Same value should produce same hash (content-addressed)
        assert_eq!(first_hash, second_hash);

        // Value should be in store only once
        assert_eq!(store.get(&first_hash).unwrap(), 42);
    }
}
