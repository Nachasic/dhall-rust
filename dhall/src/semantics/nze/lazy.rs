use core::cell::{Cell, RefCell};
use core::fmt::Debug;
use core::ops::Deref;

pub trait Eval<Tgt> {
    fn eval(self) -> Tgt;
}

/// A value which is initialized from a `Src` on the first access.
pub struct Lazy<Src, Tgt> {
    /// Exactly one of `src` of `tgt` must be set at a given time.
    /// Once `src` is unset and `tgt` is set, we never go back.
    src: Cell<Option<Src>>,
    tgt: RefCell<Option<Tgt>>,
}

impl<Src, Tgt> Lazy<Src, Tgt>
where
    Src: Eval<Tgt>,
{
    /// Creates a new lazy value with the given initializing value.
    pub fn new(src: Src) -> Self {
        Lazy {
            src: Cell::new(Some(src)),
            tgt: RefCell::new(None),
        }
    }
    /// Creates a new lazy value with the given already-initialized value.
    pub fn new_completed(tgt: Tgt) -> Self {
        Lazy {
            src: Cell::new(None),
            tgt: RefCell::new(Some(tgt)),
        }
    }

    pub fn force(&self) -> &Tgt {
        // Initialize if needed
        if self.tgt.borrow().is_none() {
            let src = self.src.take().unwrap();
            *self.tgt.borrow_mut() = Some(src.eval());
        }
        let ptr = self.tgt.as_ptr();
        // SAFETY: We just ensured the Option is Some, and the value is never
        // removed once set. The RefCell is not mutably borrowed at this point.
        unsafe { (*ptr).as_ref().unwrap() }
    }

    pub fn get_mut(&mut self) -> &mut Tgt {
        self.force();
        self.tgt.get_mut().as_mut().unwrap()
    }
    pub fn into_inner(self) -> Tgt {
        self.force();
        self.tgt.into_inner().unwrap()
    }
}

impl<Src, Tgt> Deref for Lazy<Src, Tgt>
where
    Src: Eval<Tgt>,
{
    type Target = Tgt;
    fn deref(&self) -> &Self::Target {
        self.force()
    }
}

/// This implementation evaluates before cloning, because we can't clone the contents of a `Cell`.
impl<Src, Tgt> Clone for Lazy<Src, Tgt>
where
    Src: Eval<Tgt>,
    Tgt: Clone,
{
    fn clone(&self) -> Self {
        Self::new_completed(self.force().clone())
    }
}

impl<Src, Tgt> Debug for Lazy<Src, Tgt>
where
    Tgt: Debug,
{
    fn fmt(&self, fmt: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let borrow = self.tgt.borrow();
        if let Some(tgt) = borrow.as_ref() {
            fmt.debug_tuple("Lazy@Init").field(tgt).finish()
        } else {
            fmt.debug_tuple("Lazy@Uninit").finish()
        }
    }
}
