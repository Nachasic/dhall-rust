use std::collections::HashMap;
use std::path::PathBuf;

use dhall_engine::{types::*, Engine};

/// An in-memory fetcher that controls both path resolution and content fetching.
/// Paths are stored and looked up as-is — no filesystem canonicalization.
struct InMemoryFetcher(HashMap<PathBuf, String>);

impl ImportFetcher for InMemoryFetcher {
    fn chain(
        &self,
        _base: &ImportLocation,
        import: &dhall::semantics::Import,
    ) -> Option<Result<ImportLocation, dhall::error::Error>> {
        // For local imports, resolve to our own canonical path (just the
        // file_path components joined) instead of the filesystem-based default.
        match &import.location {
            dhall::syntax::ImportTarget::Local(_prefix, file_path) => {
                let path: PathBuf = file_path.file_path.iter().collect();
                Some(Ok(ImportLocation::local(path, import.mode)))
            }
            _ => None,
        }
    }

    fn fetch(&self, location: &ImportLocation) -> Option<Result<String, dhall::error::Error>> {
        match location.kind() {
            ImportLocationKind::Local(path) => {
                // Direct lookup — paths match because we control chain().
                self.0.get(path).map(|s| Ok(s.clone()))
            }
            _ => None,
        }
    }
}

#[test]
fn test_in_memory_import() {
    let mut files = HashMap::new();
    files.insert(
        PathBuf::from("config.dhall"),
        r#"{ port = 8080, host = "localhost" }"#.to_string(),
    );

    let engine = Engine::new().with_fetcher(InMemoryFetcher(files));

    let result = engine
        .eval_str(r#"let config = ./config.dhall in config.port"#)
        .unwrap();

    assert_eq!(result.to_string(), "8080");
}

#[test]
fn test_in_memory_nested_import() {
    let mut files = HashMap::new();
    files.insert(
        PathBuf::from("types.dhall"),
        "{ Config = { port : Natural, host : Text } }".to_string(),
    );
    files.insert(
        PathBuf::from("config.dhall"),
        r#"let T = ./types.dhall in { port = 8080, host = "localhost" } : T.Config"#.to_string(),
    );

    let engine = Engine::new().with_fetcher(InMemoryFetcher(files));

    let result = engine
        .eval_str(r#"let config = ./config.dhall in config.port"#)
        .unwrap();

    assert_eq!(result.to_string(), "8080");
}
