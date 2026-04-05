//! Conversion traits between Rust types and Dhall's NIR representation.
//!
//! These traits serve three purposes:
//!
//! 1. [`DhallType`] — produce the Dhall type expression as a string
//!    (e.g. `"Natural"`, `"{ name : Text, src : Text }"`).
//! 2. [`FromNir`] — extract a Rust value from a normalized `Nir`.
//! 3. [`IntoNir`] — construct a `Nir` from a Rust value.
//!
//! Implement these for your input/output structs to avoid manual
//! `NirKind` pattern matching. Primitive types (`String`, `bool`,
//! `u64`, `i64`, `f64`) are provided.
//!
//! # Example
//!
//! ```ignore
//! struct BuildInput { name: String, src: String }
//!
//! impl DhallType for BuildInput {
//!     fn dhall_type() -> String { "{ name : Text, src : Text }".into() }
//! }
//!
//! impl FromNir for BuildInput {
//!     fn from_nir(nir: &Nir<'_>) -> Option<Self> {
//!         let f = nir.as_record()?;
//!         Some(Self {
//!             name: f.get_as("name")?,
//!             src:  f.get_as("src")?,
//!         })
//!     }
//! }
//! ```

use hashbrown::HashMap;

use dhall::semantics::{Nir, NirKind};
use dhall::syntax::{Label, NaiveDouble, NumKind};

// ── Core traits ──────────────────────────────────────────────────────

/// Produce the Dhall type expression for a Rust type.
pub trait DhallType {
    fn dhall_type() -> String;
}

/// Extract a Rust value from a normalized Nir.
pub trait FromNir: Sized {
    fn from_nir(nir: &Nir<'_>) -> Option<Self>;
}

/// Construct a Nir from a Rust value.
pub trait IntoNir {
    fn into_nir<'cx>(self) -> Nir<'cx>;
}

// ── Record field helper ──────────────────────────────────────────────

/// Wrapper around a record's fields for ergonomic extraction.
pub struct NirRecord<'a, 'cx>(pub &'a HashMap<Label, Nir<'cx>>);

impl<'a, 'cx> NirRecord<'a, 'cx> {
    /// Extract a field and convert it via `FromNir`.
    pub fn get_as<T: FromNir>(&self, key: &str) -> Option<T> {
        T::from_nir(self.0.get(&Label::from(key))?)
    }
}

/// Extension method on Nir to get a record wrapper.
pub trait NirExt<'cx> {
    fn as_record(&self) -> Option<NirRecord<'_, 'cx>>;
}

impl<'cx> NirExt<'cx> for Nir<'cx> {
    fn as_record(&self) -> Option<NirRecord<'_, 'cx>> {
        match self.kind() {
            NirKind::RecordLit(f) => Some(NirRecord(f)),
            _ => None,
        }
    }
}

// ── Primitive impls ──────────────────────────────────────────────────

// --- String / Text ---

impl DhallType for String {
    fn dhall_type() -> String { "Text".into() }
}

impl FromNir for String {
    fn from_nir(nir: &Nir<'_>) -> Option<Self> {
        match nir.kind() {
            NirKind::TextLit(txt) => txt.as_text(),
            _ => None,
        }
    }
}

impl IntoNir for String {
    fn into_nir<'cx>(self) -> Nir<'cx> { Nir::from_text(self) }
}

impl IntoNir for &str {
    fn into_nir<'cx>(self) -> Nir<'cx> { Nir::from_text(self) }
}

// --- bool ---

impl DhallType for bool {
    fn dhall_type() -> String { "Bool".into() }
}

impl FromNir for bool {
    fn from_nir(nir: &Nir<'_>) -> Option<Self> {
        match nir.kind() {
            NirKind::Num(NumKind::Bool(b)) => Some(*b),
            _ => None,
        }
    }
}

impl IntoNir for bool {
    fn into_nir<'cx>(self) -> Nir<'cx> {
        Nir::from_kind(NirKind::Num(NumKind::Bool(self)))
    }
}

// --- u64 / Natural ---

impl DhallType for u64 {
    fn dhall_type() -> String { "Natural".into() }
}

impl FromNir for u64 {
    fn from_nir(nir: &Nir<'_>) -> Option<Self> {
        match nir.kind() {
            NirKind::Num(NumKind::Natural(n)) => Some(*n),
            _ => None,
        }
    }
}

impl IntoNir for u64 {
    fn into_nir<'cx>(self) -> Nir<'cx> {
        Nir::from_kind(NirKind::Num(NumKind::Natural(self)))
    }
}

// --- i64 / Integer ---

impl DhallType for i64 {
    fn dhall_type() -> String { "Integer".into() }
}

impl FromNir for i64 {
    fn from_nir(nir: &Nir<'_>) -> Option<Self> {
        match nir.kind() {
            NirKind::Num(NumKind::Integer(n)) => Some(*n),
            _ => None,
        }
    }
}

impl IntoNir for i64 {
    fn into_nir<'cx>(self) -> Nir<'cx> {
        Nir::from_kind(NirKind::Num(NumKind::Integer(self)))
    }
}

// --- f64 / Double ---

impl DhallType for f64 {
    fn dhall_type() -> String { "Double".into() }
}

impl FromNir for f64 {
    fn from_nir(nir: &Nir<'_>) -> Option<Self> {
        match nir.kind() {
            NirKind::Num(NumKind::Double(d)) => Some((*d).into()),
            _ => None,
        }
    }
}

impl IntoNir for f64 {
    fn into_nir<'cx>(self) -> Nir<'cx> {
        Nir::from_kind(NirKind::Num(NumKind::Double(NaiveDouble::from(self))))
    }
}

// ── Record builder ───────────────────────────────────────────────────

/// Helper for constructing a `NirKind::RecordLit` from Rust values.
///
/// ```ignore
/// NirRecordBuilder::new()
///     .field("hash", hash_string)
///     .field("size", 42u64)
///     .build()
/// ```
pub struct NirRecordBuilder<'cx> {
    fields: HashMap<Label, Nir<'cx>>,
}

impl<'cx> NirRecordBuilder<'cx> {
    pub fn new() -> Self {
        Self { fields: HashMap::new() }
    }

    pub fn field(mut self, key: &str, value: impl IntoNir) -> Self {
        self.fields.insert(Label::from(key), value.into_nir());
        self
    }

    pub fn build(self) -> Nir<'cx> {
        Nir::from_kind(NirKind::RecordLit(self.fields))
    }
}

impl<'cx> Default for NirRecordBuilder<'cx> {
    fn default() -> Self { Self::new() }
}
