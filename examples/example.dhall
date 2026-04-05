-- Example derivation file
--
-- This demonstrates how to use the schema to define derivations.
-- Process it with: cargo run --example dhall_integration

let schema = ./schema.dhall

in
  { -- Simple number derivation
    answer = schema.mkNumber 42
    
    -- Another number
  , hundred = schema.mkNumber 100
    
    -- Duplicate value (will have same hash as "answer")
  , duplicate = schema.mkNumber 42
    
    -- Using mkDerivation directly
  , custom = schema.mkDerivation "number" 999
    
    -- Computed value
  , computed = schema.mkNumber (10 * 10)
  }
