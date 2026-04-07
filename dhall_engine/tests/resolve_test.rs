use std::collections::HashMap;

use dhall_engine::{types::*, Engine};

/// An in-memory fetcher that controls both path resolution and content fetching.
/// Paths are stored and looked up as-is — no filesystem canonicalization.
struct InMemoryFetcher(HashMap<LocalPath, String>);

impl ImportFetcher for InMemoryFetcher {
    fn chain(
        &self,
        _base: &ImportLocation,
        import: &dhall::semantics::Import,
    ) -> Result<ImportLocation, dhall::error::Error> {
        match &import.location {
            dhall::syntax::ImportTarget::Local(_prefix, file_path) => {
                let path: LocalPath = file_path.file_path.join("/").into();
                Ok(ImportLocation::local(path, import.mode))
            }
            _ => Err(dhall::error::ImportError::Missing.into()),
        }
    }

    fn fetch(&self, location: &ImportLocation) -> Result<String, dhall::error::Error> {
        match location.kind() {
            ImportLocationKind::Local(path) => {
                self.0.get(path).cloned().ok_or_else(|| dhall::error::ImportError::Missing.into())
            }
            _ => Err(dhall::error::ImportError::Missing.into()),
        }
    }
}

#[test]
fn test_in_memory_import() {
    let mut files = HashMap::new();
    files.insert(
        LocalPath::from("config.dhall"),
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
        LocalPath::from("types.dhall"),
        "{ Config = { port : Natural, host : Text } }".to_string(),
    );
    files.insert(
        LocalPath::from("config.dhall"),
        r#"let T = ./types.dhall in { port = 8080, host = "localhost" } : T.Config"#.to_string(),
    );

    let engine = Engine::new().with_fetcher(InMemoryFetcher(files));

    let result = engine
        .eval_str(r#"let config = ./config.dhall in config.port"#)
        .unwrap();

    assert_eq!(result.to_string(), "8080");
}
