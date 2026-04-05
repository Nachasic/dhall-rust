-- Dhall schema for the derivation system
--
-- This file defines the types and helper functions for creating derivations.
-- Import it in your derivation files:
--
--   let schema = ./schema.dhall
--   in schema.mkNumber 42

let DerivationInput = 
  { type : Text
  , value : Natural
  }

let DerivationOutput =
  { type : Text
  , hash : Text
  }

-- Helper function to create a number derivation
let mkNumber : Natural -> DerivationInput =
  \(n : Natural) ->
    { type = "number"
    , value = n
    }

-- Helper function to create a derivation with explicit type
let mkDerivation : Text -> Natural -> DerivationInput =
  \(derivationType : Text) ->
  \(value : Natural) ->
    { type = derivationType
    , value = value
    }

in
  { DerivationInput
  , DerivationOutput
  , mkNumber
  , mkDerivation
  }
