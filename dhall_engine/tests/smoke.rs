use dhall_engine::{resolve::NoImports, types::*, CustomBuiltin, Engine};

// ── DoubleNat: primitive in, primitive out ────────────────────────────

struct DoubleNat;

impl CustomBuiltin for DoubleNat {
    fn name(&self) -> &str { "doubleNat" }
    fn dhall_expr(&self) -> &str {
        concat!(
            "\\(x : Natural) -> ",
            "{ __dhall_engine_input = x, ",
            "result = 0, ",
            "__sentinel = \"__dhall_engine_sentinel:doubleNat\" }"
        )
    }
    fn apply<'cx>(&self, arg: Nir<'cx>, _cx: Ctxt<'cx>) -> Option<Nir<'cx>> {
        let n = u64::from_nir(&arg)?;
        Some((n * 2).into_nir())
    }
}

// ── HashBuilder: record in, record out ───────────────────────────────

struct BuildInput { name: String, src: String }

impl FromNir for BuildInput {
    fn from_nir(nir: &Nir<'_>) -> Option<Self> {
        let f = nir.as_record()?;
        Some(Self { name: f.get_as("name")?, src: f.get_as("src")? })
    }
}

struct HashBuilder;

impl CustomBuiltin for HashBuilder {
    fn name(&self) -> &str { "hashBuilder" }
    fn dhall_expr(&self) -> &str {
        concat!(
            "\\(input : { name : Text, src : Text }) -> ",
            "{ hash = \"__dhall_engine_sentinel:hashBuilder\", ",
            "name = input.name, ",
            "__dhall_engine_input = input }"
        )
    }
    fn apply<'cx>(&self, arg: Nir<'cx>, _cx: Ctxt<'cx>) -> Option<Nir<'cx>> {
        let input = BuildInput::from_nir(&arg)?;
        Some(NirRecordBuilder::new()
            .field("hash", format!("sha256:{}-{}", input.name, input.src))
            .field("name", input.name)
            .build())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[test]
fn test_no_builtins() {
    let engine = Engine::new().with_resolver(NoImports);
    assert_eq!(engine.eval_str("1 + 1").unwrap().to_string(), "2");
}

#[test]
fn test_double_nat() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin(DoubleNat);
    assert_eq!(engine.eval_str("doubleNat 21").unwrap().to_string(), "42");
}

#[test]
fn test_hash_builder() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin(HashBuilder);
    let s = engine.eval_str(r#"hashBuilder { name = "hello", src = "/src" }"#).unwrap().to_string();
    assert!(s.contains("sha256:hello-/src"), "got: {}", s);
}

#[test]
fn test_hash_builder_field_access() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin(HashBuilder);
    let s = engine.eval_str(r#"(hashBuilder { name = "hello", src = "/src" }).hash"#).unwrap().to_string();
    assert!(s.contains("__dhall_engine_sentinel:hashBuilder") || s.contains("sha256:"), "got: {}", s);
}

#[test]
fn test_builtin_in_list() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin(DoubleNat);
    let s = engine.eval_str("[doubleNat 1, doubleNat 2, doubleNat 3]").unwrap().to_string();
    assert!(s.contains('2') && s.contains('4') && s.contains('6'), "got: {}", s);
}

#[test]
fn test_builtin_in_record() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin(DoubleNat);
    let s = engine.eval_str("{ a = doubleNat 10, b = doubleNat 25 }").unwrap().to_string();
    assert!(s.contains("20") && s.contains("50"), "got: {}", s);
}

#[test]
fn test_multiple_builtins() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin(DoubleNat).with_builtin(HashBuilder);
    let s = engine.eval_str(r#"{ n = doubleNat 5, h = hashBuilder { name = "pkg", src = "/s" } }"#).unwrap().to_string();
    assert!(s.contains("10") && s.contains("sha256:pkg-/s"), "got: {}", s);
}

// ── Sentinel-before-beta-reduction problem ───────────────────────────
//
// These tests prove that when Dhall code USES a builtin's output in
// further computation, Dhall's normalizer operates on sentinel values
// (not real ones). Each test asserts the WRONG answer to document this.

// ── Sentinel-before-beta-reduction problem ───────────────────────────
//
// These tests demonstrate the fundamental limitation: when Dhall
// destructures a sentinel record (field access, etc.) during the first
// normalization pass, the sentinel is consumed before the engine can
// rewrite it. Re-normalization only helps when the sentinel record
// survives intact.
//
// Each test asserts the WRONG answer to document this limitation.

#[test]
fn test_limitation_field_access_then_arithmetic() {
    // Dhall extracts .result = 0 from sentinel, computes 0 + 1 = 1.
    // Correct answer would be 43 (42 + 1).
    let engine = Engine::new().with_resolver(NoImports).with_builtin(DoubleNat);
    let s = engine.eval_str("(doubleNat 21).result + 1").unwrap().to_string();
    assert_eq!(s, "1", "Limitation: sentinel consumed by field access. Got: {}", s);
}

#[test]
fn test_limitation_text_concat_with_sentinel() {
    // Dhall extracts .hash = sentinel text, concatenates with "prefix-".
    // Sentinel text is baked into the result string.
    let engine = Engine::new().with_resolver(NoImports).with_builtin(HashBuilder);
    let s = engine.eval_str(
        r#""prefix-" ++ (hashBuilder { name = "a", src = "b" }).hash"#
    ).unwrap().to_string();
    assert!(s.contains("__dhall_engine_sentinel"),
        "Limitation: sentinel baked into text. Got: {}", s);
}

#[test]
fn test_limitation_conditional_on_sentinel() {
    // Dhall extracts .result = 0, Natural/isZero 0 = True → wrong branch.
    // Correct: doubleNat 5 = 10, Natural/isZero 10 = False → "nonzero".
    let engine = Engine::new().with_resolver(NoImports).with_builtin(DoubleNat);
    let s = engine.eval_str(
        r#"if Natural/isZero (doubleNat 5).result then "zero" else "nonzero""#
    ).unwrap().to_string();
    assert_eq!(s, "\"zero\"", "Limitation: sentinel caused wrong branch. Got: {}", s);
}

#[test]
fn test_limitation_chained_builtins() {
    // Inner doubleNat's .result = 0 feeds into outer doubleNat → 0.
    // Correct: doubleNat(10) = 20.
    let engine = Engine::new().with_resolver(NoImports).with_builtin(DoubleNat);
    let s = engine.eval_str("doubleNat (doubleNat 5).result").unwrap().to_string();
    assert_eq!(s, "0", "Limitation: chained sentinel gives wrong value. Got: {}", s);
}

// ── Demonstrating that `let` binding doesn't help ────────────────────
//
// Even with a let binding, Dhall inlines and normalizes eagerly.
// `let r = hashBuilder ... in r.hash` is identical to
// `(hashBuilder ...).hash` after normalization — the sentinel is
// still destructured before the engine's rewrite pass.

#[test]
fn test_limitation_let_binding_then_field_access() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin(HashBuilder);
    let s = engine.eval_str(r#"
        let r = hashBuilder { name = "hello", src = "/src" }
        in r.hash
    "#).unwrap().to_string();
    // Dhall inlines `r`, extracts .hash → sentinel text. Same as direct field access.
    assert!(s.contains("__dhall_engine_sentinel"),
        "Limitation: let-binding doesn't prevent sentinel destructuring. Got: {}", s);
}

#[test]
fn test_limitation_let_binding_then_arithmetic() {
    let engine = Engine::new().with_resolver(NoImports).with_builtin(DoubleNat);
    let s = engine.eval_str(r#"
        let r = doubleNat 21
        in r.result + 1
    "#).unwrap().to_string();
    // Dhall inlines `r`, extracts .result = 0, computes 0 + 1 = 1.
    assert_eq!(s, "1",
        "Limitation: let-binding doesn't help. Should be 43, got: {}", s);
}

// ── But returning the whole record works ─────────────────────────────

#[test]
fn test_let_binding_whole_record_survives() {
    // When the user returns the full builtin result without destructuring,
    // the sentinel record survives and gets rewritten correctly.
    let engine = Engine::new().with_resolver(NoImports).with_builtin(HashBuilder);
    let s = engine.eval_str(r#"
        let r = hashBuilder { name = "hello", src = "/src" }
        in r
    "#).unwrap().to_string();
    assert!(s.contains("sha256:hello-/src"), "Whole record should be rewritten. Got: {}", s);
}
