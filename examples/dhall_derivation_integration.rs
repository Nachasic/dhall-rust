// Complete working example: Dhall + Derivation System
//
// This shows how to:
// 1. Define derivations in Dhall
// 2. Parse and evaluate Dhall code
// 3. Extract derivation configs
// 4. Store values content-addressed
// 5. Return hash references back to Dhall

use serde::Deserialize;
use std::collections::HashMap;

// ============================================================================
// Store Implementation
// ============================================================================

type StoreHash = String;

#[derive(Clone, Debug)]
struct StoreValue {
    value: u64,
}

struct Store {
    data: HashMap<StoreHash, StoreValue>,
}

impl Store {
    fn new() -> Self {
        Store {
            data: HashMap::new(),
        }
    }

    fn put(&mut self, value: u64) -> StoreHash {
        let hash_bytes = blake3::hash(&value.to_le_bytes());
        let hash = format!("/nix/store/{}", hex::encode(hash_bytes.as_bytes()));
        self.data.insert(hash.clone(), StoreValue { value });
        hash
    }

    fn get(&self, hash: &str) -> Option<u64> {
        self.data.get(hash).map(|v| v.value)
    }
}

// ============================================================================
// Derivation Types (matching Dhall schema)
// ============================================================================

/// Input to mkDerivation (from Dhall)
#[derive(Debug, Deserialize)]
struct DerivationInput {
    #[serde(rename = "type")]
    value_type: String,
    value: u64,
}

/// Output from mkDerivation (to Dhall)
#[derive(Debug, Clone)]
struct DerivationOutput {
    value_type: String,
    hash: StoreHash,
}

// ============================================================================
// Runtime
// ============================================================================

struct DerivationRuntime {
    store: Store,
}

impl DerivationRuntime {
    fn new() -> Self {
        DerivationRuntime {
            store: Store::new(),
        }
    }

    fn mk_derivation(&mut self, input: DerivationInput) -> DerivationOutput {
        // Only support "number" type for now
        assert_eq!(input.value_type, "number", "Only 'number' type supported");

        // Store the value
        let hash = self.store.put(input.value);

        DerivationOutput {
            value_type: input.value_type,
            hash,
        }
    }

    fn realize(&self, output: &DerivationOutput) -> Option<u64> {
        self.store.get(&output.hash)
    }
}

// ============================================================================
// Dhall Integration
// ============================================================================

/// Process Dhall derivations:
/// 1. Parse Dhall code
/// 2. Extract derivation inputs
/// 3. Call mk_derivation for each
/// 4. Return outputs with hashes
fn process_dhall_derivations(
    dhall_code: &str,
    runtime: &mut DerivationRuntime,
) -> Result<HashMap<String, DerivationOutput>, Box<dyn std::error::Error>> {
    // Parse Dhall
    let parsed: HashMap<String, DerivationInput> = serde_dhall::from_str(dhall_code).parse()?;

    // Process each derivation
    let mut outputs = HashMap::new();
    for (name, input) in parsed {
        let output = runtime.mk_derivation(input);
        outputs.insert(name, output);
    }

    Ok(outputs)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_derivation() {
        let mut runtime = DerivationRuntime::new();

        let dhall_code = r#"
            {
                myNumber = { type = "number", value = 42 }
            }
        "#;

        let outputs = process_dhall_derivations(dhall_code, &mut runtime).unwrap();

        // Check we got the derivation
        let drv = outputs.get("myNumber").unwrap();
        assert_eq!(drv.value_type, "number");
        assert!(drv.hash.starts_with("/nix/store/"));

        // Verify value is in store
        let value = runtime.realize(drv).unwrap();
        assert_eq!(value, 42);
    }

    #[test]
    fn test_multiple_derivations() {
        let mut runtime = DerivationRuntime::new();

        let dhall_code = r#"
            {
                first = { type = "number", value = 100 },
                second = { type = "number", value = 200 }
            }
        "#;

        let outputs = process_dhall_derivations(dhall_code, &mut runtime).unwrap();

        assert_eq!(outputs.len(), 2);

        let first = outputs.get("first").unwrap();
        let second = outputs.get("second").unwrap();

        assert_eq!(runtime.realize(first).unwrap(), 100);
        assert_eq!(runtime.realize(second).unwrap(), 200);

        // Different values = different hashes
        assert_ne!(first.hash, second.hash);
    }

    #[test]
    fn test_content_addressed_deduplication() {
        let mut runtime = DerivationRuntime::new();

        let dhall_code = r#"
            {
                drv1 = { type = "number", value = 42 },
                drv2 = { type = "number", value = 42 }
            }
        "#;

        let outputs = process_dhall_derivations(dhall_code, &mut runtime).unwrap();

        let drv1 = outputs.get("drv1").unwrap();
        let drv2 = outputs.get("drv2").unwrap();

        // Same value = same hash
        assert_eq!(drv1.hash, drv2.hash);

        // Store only has one entry
        assert_eq!(runtime.store.data.len(), 1);
    }

    #[test]
    fn test_with_dhall_functions() {
        let mut runtime = DerivationRuntime::new();

        // Dhall with a helper function
        let dhall_code = r#"
            let mkNumber = \(n : Natural) -> { type = "number", value = n }
            in {
                small = mkNumber 10,
                large = mkNumber 1000
            }
        "#;

        let outputs = process_dhall_derivations(dhall_code, &mut runtime).unwrap();

        assert_eq!(outputs.len(), 2);
        assert_eq!(runtime.realize(outputs.get("small").unwrap()).unwrap(), 10);
        assert_eq!(runtime.realize(outputs.get("large").unwrap()).unwrap(), 1000);
    }

    #[test]
    fn test_lazy_evaluation_only_needed_fields() {
        let mut runtime = DerivationRuntime::new();

        // In real usage, you'd only parse the field you need
        // This simulates accessing only "needed"
        let dhall_code = r#"
            {
                needed = { type = "number", value = 42 },
                unused = { type = "number", value = 999 }
            }
        "#;

        let outputs = process_dhall_derivations(dhall_code, &mut runtime).unwrap();

        // In a real lazy system, we'd only process "needed"
        // For this test, we verify both are processed but we only realize one
        let needed = outputs.get("needed").unwrap();

        // Only realize the needed derivation
        assert_eq!(runtime.realize(needed).unwrap(), 42);

        // Both are in store (because we processed all)
        // In a truly lazy system, "unused" wouldn't be in the store yet
        assert_eq!(runtime.store.data.len(), 2);
    }
}

// ============================================================================
// Main
// ============================================================================

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = DerivationRuntime::new();

    println!("=== Dhall Derivation System ===\n");

    // Example Dhall code
    let dhall_code = r#"
        let mkNumber = \(n : Natural) -> { type = "number", value = n }
        
        in {
            answer = mkNumber 42,
            hundred = mkNumber 100,
            duplicate = mkNumber 42
        }
    "#;

    println!("Dhall code:\n{}\n", dhall_code);

    // Process derivations
    println!("Processing derivations...");
    let outputs = process_dhall_derivations(dhall_code, &mut runtime)?;

    // Display results
    println!("\nDerivations created:");
    for (name, output) in &outputs {
        println!("  {}: {} -> {}", name, output.value_type, output.hash);
    }

    // Realize derivations
    println!("\nRealizing derivations:");
    for (name, output) in &outputs {
        let value = runtime.realize(output).unwrap();
        println!("  {}: {}", name, value);
    }

    println!("\nStore statistics:");
    println!("  Unique values: {}", runtime.store.data.len());
    println!("  (Note: 'answer' and 'duplicate' share the same hash)");

    Ok(())
}
