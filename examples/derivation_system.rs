// Minimal working example of a Nix-like derivation system using Dhall
//
// This demonstrates:
// 1. Dhall code defines derivations as pure data
// 2. Rust evaluates derivations lazily
// 3. Values are stored content-addressed by hash
// 4. Same input = same hash (deduplication)

use std::collections::HashMap;

// ============================================================================
// Store Implementation
// ============================================================================

type StoreHash = String;

#[derive(Clone, Debug, PartialEq)]
struct StoreValue {
    value_type: String,
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

    fn put(&mut self, value: StoreValue) -> StoreHash {
        // Content-addressed: hash the value itself
        let content = format!("{}:{}", value.value_type, value.value);
        let hash_bytes = blake3::hash(content.as_bytes());
        let hash = format!("/nix/store/{}", hex::encode(hash_bytes.as_bytes()));
        
        self.data.insert(hash.clone(), value);
        hash
    }

    fn get(&self, hash: &str) -> Option<&StoreValue> {
        self.data.get(hash)
    }
}

// ============================================================================
// Derivation Runtime
// ============================================================================

/// A derivation as returned by mkDerivation
#[derive(Debug, Clone, PartialEq)]
struct Derivation {
    value_type: String,
    hash: StoreHash,
}

struct DerivationRuntime {
    store: Store,
}

impl DerivationRuntime {
    fn new() -> Self {
        DerivationRuntime {
            store: Store::new(),
        }
    }

    /// Simulates Runtime/mkDerivation
    /// Takes { type : Text, value : Natural }
    /// Returns { type : Text, hash : Text }
    fn mk_derivation(&mut self, value_type: String, value: u64) -> Derivation {
        let store_value = StoreValue { value_type: value_type.clone(), value };
        let hash = self.store.put(store_value);
        
        Derivation { value_type, hash }
    }

    /// Retrieve a value from the store by hash
    fn realize(&self, drv: &Derivation) -> Option<u64> {
        self.store.get(&drv.hash).map(|v| v.value)
    }
}

// ============================================================================
// Dhall Integration (Simulated)
// ============================================================================

/// Simulates parsing and evaluating this Dhall code:
/// ```dhall
/// let mkDerivation = \(config : { type : Text, value : Natural }) ->
///   { type = config.type, value = config.value }
///
/// in {
///   drv1 = mkDerivation { type = "number", value = 42 },
///   drv2 = mkDerivation { type = "number", value = 100 }
/// }
/// ```
fn simulate_dhall_evaluation(runtime: &mut DerivationRuntime) -> HashMap<String, Derivation> {
    let mut result = HashMap::new();
    
    // Simulate: drv1 = mkDerivation { type = "number", value = 42 }
    result.insert(
        "drv1".to_string(),
        runtime.mk_derivation("number".to_string(), 42),
    );
    
    // Simulate: drv2 = mkDerivation { type = "number", value = 100 }
    result.insert(
        "drv2".to_string(),
        runtime.mk_derivation("number".to_string(), 100),
    );
    
    result
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mk_derivation_stores_value() {
        let mut runtime = DerivationRuntime::new();
        
        // Create a derivation
        let drv = runtime.mk_derivation("number".to_string(), 42);
        
        // Check derivation structure
        assert_eq!(drv.value_type, "number");
        assert!(drv.hash.starts_with("/nix/store/"));
        
        // Verify value is in store
        let stored = runtime.realize(&drv).unwrap();
        assert_eq!(stored, 42);
    }

    #[test]
    fn test_same_value_produces_same_hash() {
        let mut runtime = DerivationRuntime::new();
        
        // Create two derivations with same value
        let drv1 = runtime.mk_derivation("number".to_string(), 42);
        let drv2 = runtime.mk_derivation("number".to_string(), 42);
        
        // Should have same hash (content-addressed)
        assert_eq!(drv1.hash, drv2.hash);
        
        // Store should only have one entry
        assert_eq!(runtime.store.data.len(), 1);
    }

    #[test]
    fn test_different_values_produce_different_hashes() {
        let mut runtime = DerivationRuntime::new();
        
        let drv1 = runtime.mk_derivation("number".to_string(), 42);
        let drv2 = runtime.mk_derivation("number".to_string(), 100);
        
        // Different hashes
        assert_ne!(drv1.hash, drv2.hash);
        
        // Both values in store
        assert_eq!(runtime.realize(&drv1).unwrap(), 42);
        assert_eq!(runtime.realize(&drv2).unwrap(), 100);
    }

    #[test]
    fn test_dhall_simulation() {
        let mut runtime = DerivationRuntime::new();
        
        // Simulate Dhall evaluation
        let derivations = simulate_dhall_evaluation(&mut runtime);
        
        // Check we got both derivations
        assert_eq!(derivations.len(), 2);
        
        let drv1 = derivations.get("drv1").unwrap();
        let drv2 = derivations.get("drv2").unwrap();
        
        // Verify values
        assert_eq!(runtime.realize(drv1).unwrap(), 42);
        assert_eq!(runtime.realize(drv2).unwrap(), 100);
        
        // Verify hashes are different
        assert_ne!(drv1.hash, drv2.hash);
    }

    #[test]
    fn test_lazy_evaluation_simulation() {
        let mut runtime = DerivationRuntime::new();
        
        // Simulate: only evaluate drv1, not drv2
        let drv1 = runtime.mk_derivation("number".to_string(), 42);
        
        // At this point, drv2 would not be evaluated in real Dhall
        // (it would remain a thunk)
        
        // Only drv1 is in the store
        assert_eq!(runtime.store.data.len(), 1);
        assert_eq!(runtime.realize(&drv1).unwrap(), 42);
    }
}

// ============================================================================
// Main (demonstrates usage)
// ============================================================================

fn main() {
    let mut runtime = DerivationRuntime::new();
    
    println!("=== Derivation System Demo ===\n");
    
    // Create derivations
    println!("Creating derivations...");
    let drv1 = runtime.mk_derivation("number".to_string(), 42);
    println!("drv1: type={}, hash={}", drv1.value_type, drv1.hash);
    
    let drv2 = runtime.mk_derivation("number".to_string(), 100);
    println!("drv2: type={}, hash={}", drv2.value_type, drv2.hash);
    
    // Create duplicate (same hash)
    let drv3 = runtime.mk_derivation("number".to_string(), 42);
    println!("drv3: type={}, hash={} (same as drv1!)", drv3.value_type, drv3.hash);
    
    // Realize derivations
    println!("\nRealizing derivations...");
    println!("drv1 value: {}", runtime.realize(&drv1).unwrap());
    println!("drv2 value: {}", runtime.realize(&drv2).unwrap());
    println!("drv3 value: {}", runtime.realize(&drv3).unwrap());
    
    println!("\nStore contents: {} unique values", runtime.store.data.len());
}
