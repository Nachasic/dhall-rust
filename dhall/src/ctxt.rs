use core::cell::RefCell;
use core::cell::Cell;
use core::marker::PhantomData;
use core::ops::{Deref, Index};
use alloc::sync::Arc;

/// Append-only vector that allows pushing through a shared reference.
/// Items are boxed so references to them remain stable.
/// Replacement for `elsa::vec::AppendVec` that doesn't require `std`-only deps.
struct AppendVec<T> {
    inner: RefCell<Vec<T>>,
}

impl<T> AppendVec<T> {
    fn new() -> Self {
        AppendVec { inner: RefCell::new(Vec::new()) }
    }
    fn len(&self) -> usize {
        self.inner.borrow().len()
    }
    fn push(&self, val: T) {
        self.inner.borrow_mut().push(val);
    }
}

impl<T> Index<usize> for AppendVec<Box<T>> {
    type Output = T;
    fn index(&self, idx: usize) -> &T {
        let borrow = self.inner.borrow();
        let ptr: *const T = &**borrow.index(idx);
        // SAFETY: Items are boxed (heap-allocated) and never removed or moved,
        // so the pointer remains valid for the lifetime of the AppendVec.
        unsafe { &*ptr }
    }
}

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::semantics::{Import, ImportLocation, ImportNode, Nir};
use crate::syntax::Span;
use crate::Typed;

/////////////////////////////////////////////////////////////////////////////////////////////////////
// Ctxt

/// A registered custom builtin: name + handler.
#[derive(Clone)]
pub struct CustomBuiltinEntry {
    pub name: String,
    pub handler: Arc<dyn for<'cx> CustomBuiltinHandler<'cx>>,
}

/// Trait for custom builtin dispatch.
pub trait CustomBuiltinHandler<'cx> {
    fn call(&self, args: &[Nir<'cx>], cx: Ctxt<'cx>) -> Option<Nir<'cx>>;
}

/// Implementation detail. Made public for the `Index` instances.
pub struct CtxtS<'cx> {
    imports: AppendVec<Box<StoredImport<'cx>>>,
    import_alternatives: AppendVec<Box<StoredImportAlternative<'cx>>>,
    import_results: AppendVec<Box<StoredImportResult<'cx>>>,
    custom_builtins: Vec<CustomBuiltinEntry>,
}

impl<'cx> Default for CtxtS<'cx> {
    fn default() -> Self {
        CtxtS {
            imports: AppendVec::new(),
            import_alternatives: AppendVec::new(),
            import_results: AppendVec::new(),
            custom_builtins: Vec::new(),
        }
    }
}

/// Context for the dhall compiler. Stores various global maps.
/// Access the relevant value using `cx[id]`.
#[derive(Copy, Clone)]
pub struct Ctxt<'cx>(&'cx CtxtS<'cx>);

impl Ctxt<'_> {
    pub fn with_new<T>(f: impl for<'cx> FnOnce(Ctxt<'cx>) -> T) -> T {
        let cx = CtxtS::default();
        let cx = core::mem::ManuallyDrop::new(cx);
        let cx = Ctxt(&cx);
        f(cx)
    }

    pub fn with_new_custom<T>(
        builtins: Vec<CustomBuiltinEntry>,
        f: impl for<'cx> FnOnce(Ctxt<'cx>) -> T,
    ) -> T {
        let mut cx = CtxtS::default();
        cx.custom_builtins = builtins;
        let cx = core::mem::ManuallyDrop::new(cx);
        let cx = Ctxt(&cx);
        f(cx)
    }
}

impl<'cx> Ctxt<'cx> {
    pub fn lookup_custom_builtin(&self, name: &str) -> Option<usize> {
        self.0.custom_builtins.iter().position(|b| b.name == name)
    }

    pub fn call_custom_builtin(&self, id: usize, args: &[Nir<'cx>]) -> Option<Nir<'cx>> {
        self.0.custom_builtins[id].handler.call(args, *self)
    }

    pub fn custom_builtin_name(&self, id: usize) -> &str {
        &self.0.custom_builtins[id].name
    }
}
impl<'cx> Deref for Ctxt<'cx> {
    type Target = &'cx CtxtS<'cx>;
    fn deref(&self) -> &&'cx CtxtS<'cx> {
        &self.0
    }
}
impl<'a, 'cx, T> Index<&'a T> for CtxtS<'cx>
where
    Self: Index<T>,
    T: Copy,
{
    type Output = <Self as Index<T>>::Output;
    fn index(&self, id: &'a T) -> &Self::Output {
        &self[*id]
    }
}

/// Empty impl, because `AppendVec` does not implement `Debug` and I can't be bothered to do it
/// myself.
impl<'cx> core::fmt::Debug for Ctxt<'cx> {
    fn fmt(&self, _: &mut core::fmt::Formatter) -> core::fmt::Result {
        Ok(())
    }
}

/// All Ctxt values within a session point to the same arena.
impl<'cx> PartialEq for Ctxt<'cx> {
    fn eq(&self, _other: &Self) -> bool { true }
}
impl<'cx> Eq for Ctxt<'cx> {}

/////////////////////////////////////////////////////////////////////////////////////////////////////
// Imports

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ImportId<'cx>(usize, PhantomData<&'cx ()>);

/// What's stored for each `ImportId`. Allows getting and setting a result for this import.
pub struct StoredImport<'cx> {
    cx: Ctxt<'cx>,
    pub base_location: ImportLocation,
    pub import: Import,
    pub span: Span,
    result: Cell<Option<ImportResultId<'cx>>>,
}

impl<'cx> StoredImport<'cx> {
    /// Get the id of the result of fetching this import. Returns `None` if the result has not yet
    /// been fetched.
    pub fn get_resultid(&self) -> Option<ImportResultId<'cx>> {
        self.result.get()
    }
    /// Store the result of fetching this import.
    pub fn set_resultid(&self, res: ImportResultId<'cx>) {
        self.result.set(Some(res));
    }
    /// Get the result of fetching this import. Returns `None` if the result has not yet been
    /// fetched.
    pub fn get_result(&self) -> Option<&'cx StoredImportResult<'cx>> {
        let res = self.get_resultid()?;
        Some(&self.cx[res])
    }
    /// Get the result of fetching this import. Panicx if the result has not yet been
    /// fetched.
    pub fn unwrap_result(&self) -> &'cx StoredImportResult<'cx> {
        self.get_result()
            .expect("imports should all have been resolved at this stage")
    }
    /// Store the result of fetching this import.
    pub fn set_result(
        &self,
        res: StoredImportResult<'cx>,
    ) -> ImportResultId<'cx> {
        let res = self.cx.push_import_result(res);
        self.set_resultid(res);
        res
    }
}
impl<'cx> Ctxt<'cx> {
    /// Store an import and the location relative to which it must be resolved.
    pub fn push_import(
        self,
        base_location: ImportLocation,
        import: Import,
        span: Span,
    ) -> ImportId<'cx> {
        let stored = StoredImport {
            cx: self,
            base_location,
            import,
            span,
            result: Cell::new(None),
        };
        let id = self.0.imports.len();
        self.0.imports.push(Box::new(stored));
        ImportId(id, PhantomData)
    }
}
impl<'cx> Index<ImportId<'cx>> for CtxtS<'cx> {
    type Output = StoredImport<'cx>;
    fn index(&self, id: ImportId<'cx>) -> &StoredImport<'cx> {
        &self.imports[id.0]
    }
}

/////////////////////////////////////////////////////////////////////////////////////////////////////
// Import alternatives

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ImportAlternativeId<'cx>(usize, PhantomData<&'cx ()>);

/// What's stored for each `ImportAlternativeId`.
pub struct StoredImportAlternative<'cx> {
    pub left_imports: Box<[ImportNode<'cx>]>,
    pub right_imports: Box<[ImportNode<'cx>]>,
    /// `true` for left, `false` for right.
    selected: Cell<Option<bool>>,
}

impl<'cx> StoredImportAlternative<'cx> {
    /// Get which alternative got selected. `true` for left, `false` for right.
    pub fn get_selected(&self) -> Option<bool> {
        self.selected.get()
    }
    /// Get which alternative got selected. `true` for left, `false` for right.
    pub fn unwrap_selected(&self) -> bool {
        self.get_selected()
            .expect("imports should all have been resolved at this stage")
    }
    /// Set which alternative got selected. `true` for left, `false` for right.
    pub fn set_selected(&self, selected: bool) {
        self.selected.set(Some(selected));
    }
}
impl<'cx> Ctxt<'cx> {
    pub fn push_import_alternative(
        self,
        left_imports: Box<[ImportNode<'cx>]>,
        right_imports: Box<[ImportNode<'cx>]>,
    ) -> ImportAlternativeId<'cx> {
        let stored = StoredImportAlternative {
            left_imports,
            right_imports,
            selected: Cell::new(None),
        };
        let id = self.0.import_alternatives.len();
        self.0.import_alternatives.push(Box::new(stored));
        ImportAlternativeId(id, PhantomData)
    }
}
impl<'cx> Index<ImportAlternativeId<'cx>> for CtxtS<'cx> {
    type Output = StoredImportAlternative<'cx>;
    fn index(
        &self,
        id: ImportAlternativeId<'cx>,
    ) -> &StoredImportAlternative<'cx> {
        &self.import_alternatives[id.0]
    }
}

/////////////////////////////////////////////////////////////////////////////////////////////////////
// Import results

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ImportResultId<'cx>(usize, PhantomData<&'cx ()>);

type StoredImportResult<'cx> = Typed<'cx>;

impl<'cx> Ctxt<'cx> {
    /// Store the result of fetching an import.
    pub fn push_import_result(
        self,
        res: StoredImportResult<'cx>,
    ) -> ImportResultId<'cx> {
        let id = self.0.import_results.len();
        self.0.import_results.push(Box::new(res));
        ImportResultId(id, PhantomData)
    }
}
impl<'cx> Index<ImportResultId<'cx>> for CtxtS<'cx> {
    type Output = StoredImportResult<'cx>;
    fn index(&self, id: ImportResultId<'cx>) -> &StoredImportResult<'cx> {
        &self.import_results[id.0]
    }
}
