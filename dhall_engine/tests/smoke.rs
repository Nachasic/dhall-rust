use dhall_engine::{resolve::NoImports, types::*, Engine};

// ── DoubleNat: Natural -> Natural ────────────────────────────────────

struct DoubleNat;

impl<'cx> CustomBuiltinHandler<'cx> for DoubleNat {
    fn call(&self, args: &[Nir<'cx>], _cx: Ctxt<'cx>) -> Option<Nir<'cx>> {
        if args.len() != 1 { return None; }
        let n = u64::from_nir(&args[0])?;
        Some((n * 2).into_nir())
    }
}

// ── HashBuilder: { name, src } -> { hash, name } ────────────────────

struct BuildInput { name: String, src: String }

impl FromNir for BuildInput {
    fn from_nir(nir: &Nir<'_>) -> Option<Self> {
        let f = nir.as_record()?;
        Some(Self { name: f.get_as("name")?, src: f.get_as("src")? })
    }
}

struct HashBuilder;

impl<'cx> CustomBuiltinHandler<'cx> for HashBuilder {
    fn call(&self, args: &[Nir<'cx>], _cx: Ctxt<'cx>) -> Option<Nir<'cx>> {
        if args.len() != 1 { return None; }
        let input = BuildInput::from_nir(&args[0])?;
        Some(NirRecordBuilder::new()
            .field("hash", format!("sha256:{}-{}", input.name, input.src))
            .field("name", input.name)
            .build())
    }
}

// ── Basic tests ──────────────────────────────────────────────────────

#[test]
fn test_no_builtins() {
    let engine = Engine::new().with_resolver(NoImports);
    assert_eq!(engine.eval_str("1 + 1").unwrap().to_string(), "2");
}

#[test]
fn test_double_nat() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("doubleNat", DoubleNat);
    assert_eq!(engine.eval_str("doubleNat 21").unwrap().to_string(), "42");
}

#[test]
fn test_hash_builder() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("hashBuilder", HashBuilder);
    let s = engine.eval_str(r#"hashBuilder { name = "hello", src = "/src" }"#).unwrap().to_string();
    assert!(s.contains("sha256:hello-/src"), "got: {}", s);
}

#[test]
fn test_builtin_in_list() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("doubleNat", DoubleNat);
    let s = engine.eval_str("[doubleNat 1, doubleNat 2, doubleNat 3]").unwrap().to_string();
    assert!(s.contains('2') && s.contains('4') && s.contains('6'), "got: {}", s);
}

#[test]
fn test_builtin_in_record() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("doubleNat", DoubleNat);
    let s = engine.eval_str("{ a = doubleNat 10, b = doubleNat 25 }").unwrap().to_string();
    assert!(s.contains("20") && s.contains("50"), "got: {}", s);
}

#[test]
fn test_multiple_builtins() {
    let engine = Engine::new()
        .with_resolver(NoImports)
        .with_builtin("doubleNat", DoubleNat)
        .with_builtin("hashBuilder", HashBuilder);
    let s = engine.eval_str(r#"{ n = doubleNat 5, h = hashBuilder { name = "pkg", src = "/s" } }"#).unwrap().to_string();
    assert!(s.contains("10") && s.contains("sha256:pkg-/s"), "got: {}", s);
}

// ── Tests that require builtins in normalization ─────────────────────

#[test]
fn test_field_access_then_arithmetic() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("doubleNat", DoubleNat);
    assert_eq!(engine.eval_str("doubleNat 21 + 1").unwrap().to_string(), "43");
}

#[test]
fn test_hash_builder_field_access() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("hashBuilder", HashBuilder);
    let s = engine.eval_str(r#"(hashBuilder { name = "hello", src = "/src" }).hash"#).unwrap().to_string();
    assert!(s.contains("sha256:hello-/src"), "got: {}", s);
}

#[test]
fn test_text_concat_with_builtin() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("hashBuilder", HashBuilder);
    let s = engine.eval_str(r#""prefix-" ++ (hashBuilder { name = "a", src = "b" }).hash"#).unwrap().to_string();
    assert!(s.contains("prefix-sha256:a-b"), "got: {}", s);
}

#[test]
fn test_conditional_on_builtin() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("doubleNat", DoubleNat);
    let s = engine.eval_str(r#"if Natural/isZero (doubleNat 5) then "zero" else "nonzero""#).unwrap().to_string();
    assert_eq!(s, "\"nonzero\"", "got: {}", s);
}

#[test]
fn test_chained_builtins() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("doubleNat", DoubleNat);
    assert_eq!(engine.eval_str("doubleNat (doubleNat 5)").unwrap().to_string(), "20");
}

#[test]
fn test_let_binding_then_field_access() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("hashBuilder", HashBuilder);
    let s = engine.eval_str(r#"let r = hashBuilder { name = "hello", src = "/src" } in r.hash"#).unwrap().to_string();
    assert!(s.contains("sha256:hello-/src"), "got: {}", s);
}

#[test]
fn test_let_binding_then_arithmetic() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin("doubleNat", DoubleNat);
    assert_eq!(engine.eval_str("let r = doubleNat 21 in r + 1").unwrap().to_string(), "43");
}
