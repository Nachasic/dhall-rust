use hashbrown::HashMap;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::error::{Error, ImportError};
#[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
use crate::semantics::Cache;
use crate::semantics::{check_hash, AlphaVar, ImportLocation, VarEnv};
use crate::syntax::{Hash, Label, V};
use crate::{Ctxt, ImportId, ImportResultId, Typed};

/// Trait for custom import fetching. Implement this to resolve imports
/// from sources other than the filesystem/HTTP (e.g. in-memory, virtual FS).
///
/// Receives the full `ImportLocation` for each import. Return `None`
/// from any method to fall back to the default behavior.
pub trait ImportFetcher {
    /// Resolve an import path relative to a base location.
    /// Return `None` to use default path resolution (filesystem-based).
    fn chain(
        &self,
        _base: &ImportLocation,
        _import: &crate::semantics::Import,
    ) -> Option<Result<ImportLocation, Error>> {
        None
    }

    /// Fetch content for a resolved location.
    /// Return `None` to use default I/O (filesystem/HTTP).
    fn fetch(&self, location: &ImportLocation) -> Option<Result<String, Error>>;
}

/// Environment for resolving names.
#[derive(Debug, Clone, Default)]
pub struct NameEnv {
    names: Vec<Label>,
}

pub type CyclesStack = Vec<ImportLocation>;

/// Environment for resolving imports
pub struct ImportEnv<'cx> {
    cx: Ctxt<'cx>,
    #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
    disk_cache: Option<Cache>,
    mem_cache: HashMap<ImportLocation, ImportResultId<'cx>>,
    stack: CyclesStack,
    fetcher: Option<Box<dyn ImportFetcher>>,
}

impl NameEnv {
    pub fn new() -> Self {
        NameEnv::default()
    }
    pub fn as_varenv(&self) -> VarEnv {
        VarEnv::from_size(self.names.len())
    }

    pub fn insert(&self, x: &Label) -> Self {
        let mut env = self.clone();
        env.insert_mut(x);
        env
    }
    pub fn insert_mut(&mut self, x: &Label) {
        self.names.push(x.clone())
    }
    pub fn remove_mut(&mut self) {
        self.names.pop();
    }

    pub fn unlabel_var(&self, var: &V) -> Option<AlphaVar> {
        let V(name, idx) = var;
        let (idx, _) = self
            .names
            .iter()
            .rev()
            .enumerate()
            .filter(|(_, n)| *n == name)
            .nth(*idx)?;
        Some(AlphaVar::new(idx))
    }
    pub fn label_var(&self, var: AlphaVar) -> V {
        let name = &self.names[self.names.len() - 1 - var.idx()];
        let idx = self
            .names
            .iter()
            .rev()
            .take(var.idx())
            .filter(|n| *n == name)
            .count();
        V(name.clone(), idx)
    }
}

impl<'cx> ImportEnv<'cx> {
    pub fn new(cx: Ctxt<'cx>) -> Self {
        ImportEnv {
            cx,
            #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
            disk_cache: Cache::new().ok(),
            mem_cache: Default::default(),
            stack: Default::default(),
            fetcher: None,
        }
    }

    pub fn with_fetcher(cx: Ctxt<'cx>, fetcher: Box<dyn ImportFetcher>) -> Self {
        ImportEnv {
            cx,
            #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
            disk_cache: Cache::new().ok(),
            mem_cache: Default::default(),
            stack: Default::default(),
            fetcher: Some(fetcher),
        }
    }

    pub fn fetcher(&self) -> Option<&dyn ImportFetcher> {
        self.fetcher.as_deref()
    }

    pub fn cx(&self) -> Ctxt<'cx> {
        self.cx
    }

    pub fn get_from_mem_cache(
        &self,
        location: &ImportLocation,
    ) -> Option<ImportResultId<'cx>> {
        Some(*self.mem_cache.get(location)?)
    }

    pub fn get_from_disk_cache(
        &self,
        hash: &Option<Hash>,
    ) -> Option<Typed<'cx>> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
        {
            let hash = hash.as_ref()?;
            let expr = self.disk_cache.as_ref()?.get(self.cx(), hash).ok()?;
            return Some(expr);
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "std")))]
        { let _ = hash; None }
    }

    pub fn check_hash(
        &self,
        import: ImportId<'cx>,
        result: ImportResultId<'cx>,
    ) -> Result<(), Error> {
        check_hash(self.cx(), import, result)
    }

    pub fn write_to_mem_cache(
        &mut self,
        location: ImportLocation,
        result: ImportResultId<'cx>,
    ) {
        self.mem_cache.insert(location, result);
    }

    pub fn write_to_disk_cache(
        &self,
        hash: &Option<Hash>,
        result: ImportResultId<'cx>,
    ) {
        #[cfg(all(not(target_arch = "wasm32"), feature = "std"))]
        if let Some(disk_cache) = self.disk_cache.as_ref() {
            if let Some(hash) = hash {
                let expr = &self.cx()[result];
                let _ = disk_cache.insert(self.cx(), hash, expr);
            }
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "std")))]
        { let _ = (hash, result); }
    }

    pub fn with_cycle_detection(
        &mut self,
        location: ImportLocation,
        do_resolve: impl FnOnce(&mut Self) -> Result<Typed<'cx>, Error>,
    ) -> Result<Typed<'cx>, Error> {
        if self.stack.contains(&location) {
            return Err(
                ImportError::ImportCycle(self.stack.clone(), location).into()
            );
        }
        // Push the current location on the stack
        self.stack.push(location);
        // Resolve the import recursively
        // WARNING: do not propagate errors here or the stack will get messed up.
        let result = do_resolve(self);
        // Remove location from the stack.
        self.stack.pop().unwrap();
        result
    }
}
