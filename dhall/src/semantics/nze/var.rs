use crate::syntax::Label;

// Exactly like a Label, but equality returns always true.
// This is so that NirKind equality is exactly alpha-equivalence.
#[derive(Clone, Eq)]
pub struct Binder {
    name: Label,
}

impl Binder {
    pub fn new(name: Label) -> Self {
        Binder { name }
    }
    pub fn to_label(&self) -> Label {
        self.clone().into()
    }
}

/// Equality up to alpha-equivalence
impl core::cmp::PartialEq for Binder {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl core::fmt::Debug for Binder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Binder({})", &self.name)
    }
}

impl From<Binder> for Label {
    fn from(x: Binder) -> Label {
        x.name
    }
}
