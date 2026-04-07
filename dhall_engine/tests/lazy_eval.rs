use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use dhall_engine::{types::*, Engine, NoImports};

/// A builtin that counts how many times it has been called.
struct CountingDouble {
    call_count: Arc<AtomicUsize>,
}

impl<'cx> CustomBuiltinHandler<'cx> for CountingDouble {
    fn call(&self, args: &[Nir<'cx>], _cx: Ctxt<'cx>) -> Option<Nir<'cx>> {
        if args.len() != 1 { return None; }
        let n = match args[0].kind() {
            NirKind::Num(NumKind::Natural(n)) => *n,
            _ => return None,
        };
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Some(Nir::from_kind(NirKind::Num(NumKind::Natural(n * 2))))
    }
}

#[test]
fn test_lazy_field_navigation() {
    let call_count = Arc::new(AtomicUsize::new(0));

    let engine = Engine::new()
        .with_fetcher(NoImports)
        .with_builtin("doubleNat", "Natural -> Natural", CountingDouble {
            call_count: Arc::clone(&call_count),
        });

    let input = r#"{ plain = 1, doubled = doubleNat 21 }"#;

    engine.eval_lazy(input, |lazy| {
        // Nothing evaluated yet.
        assert_eq!(call_count.load(Ordering::SeqCst), 0);

        // We can inspect field names without triggering any evaluation.
        let names = lazy.field_names().unwrap();
        assert!(names.contains(&"plain".to_string()));
        assert!(names.contains(&"doubled".to_string()));
        assert_eq!(call_count.load(Ordering::SeqCst), 0);

        // Evaluating "plain" does not trigger the custom builtin.
        let plain = lazy.field("plain").unwrap();
        match plain.normalize().kind() {
            NirKind::Num(NumKind::Natural(1)) => {}
            other => panic!("expected Natural(1), got: {:?}", other),
        }
        assert_eq!(call_count.load(Ordering::SeqCst), 0, "builtin not called for plain field");

        // Evaluating "doubled" triggers the custom builtin exactly once.
        let doubled = lazy.field("doubled").unwrap();
        match doubled.normalize().kind() {
            NirKind::Num(NumKind::Natural(42)) => {}
            other => panic!("expected Natural(42), got: {:?}", other),
        }
        assert_eq!(call_count.load(Ordering::SeqCst), 1, "builtin called exactly once");
    }).unwrap();
}

#[test]
fn test_lazy_full_normalize() {
    let engine = Engine::new().with_fetcher(NoImports);

    let result = engine.eval_lazy("1 + 1", |lazy| {
        lazy.to_expr().to_string()
    }).unwrap();

    assert_eq!(result, "2");
}

#[test]
fn test_lazy_nonrecord_field_names_is_none() {
    let engine = Engine::new().with_fetcher(NoImports);

    engine.eval_lazy("42", |lazy| {
        assert!(lazy.field_names().is_none());
        assert!(lazy.field("x").is_none());
    }).unwrap();
}
