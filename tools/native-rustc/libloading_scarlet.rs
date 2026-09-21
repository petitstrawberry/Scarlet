//! Experimental libloading adapter for Scarlet's eager, global, pinned loader.
//!
//! Copy into libloading's src/scarlet.rs and apply the matching export patch.
//! Libraries are always opened with NOW|GLOBAL (0x102). LOCAL, lazy binding,
//! ELF TLS and unloading are outside this bring-up contract. This intentionally
//! does not claim the full libloading platform API or Send/Sync guarantees.
use crate::Error;
use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::mem;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, TryLockError};

// Copy dlerror while serialized; its pointer is owned by the interpreter.
static CALLS: Mutex<()> = Mutex::new(());

fn calls() -> Result<MutexGuard<'static, ()>, ()> {
    // Constructors may recurse into libloading: fail instead of deadlocking.
    match CALLS.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        Err(TryLockError::WouldBlock) => Err(()),
    }
}

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *mut c_char;
}

#[derive(Debug)]
pub struct Library {
    handle: *mut c_void,
    // The initial interpreter has no general concurrent dlopen contract.
    _not_send_sync: PhantomData<Rc<()>>,
}

impl Library {
    /// # Safety
    /// The library's initializers and exported symbols must be safe to load.
    pub unsafe fn new<P: AsRef<OsStr>>(filename: P) -> Result<Self, Error> {
        let name = CString::new(filename.as_ref().as_encoded_bytes())
            .map_err(|source| Error::CreateCString { source })?;
        let _guard = calls().map_err(|_| Error::DlOpenUnknown)?;
        unsafe { dlerror() };
        let handle = unsafe { dlopen(name.as_ptr(), 0x102) };
        if handle.is_null() {
            let err = unsafe { dlerror() };
            return Err(if err.is_null() { Error::DlOpenUnknown }
                else { Error::DlOpen { desc: unsafe { CStr::from_ptr(err) }.into() } });
        }
        Ok(Self { handle, _not_send_sync: PhantomData })
    }

    /// # Safety
    /// T must be the exact pointer or function-pointer type exported by the DSO.
    pub unsafe fn get<T>(&self, name: &[u8]) -> Result<Symbol<'_, T>, Error> {
        if mem::size_of::<T>() != mem::size_of::<*mut c_void>()
            || mem::align_of::<T>() > mem::align_of::<*mut c_void>()
        {
            return Err(Error::IncompatibleSize);
        }
        let bytes = name.strip_suffix(&[0]).unwrap_or(name);
        let name = CString::new(bytes).map_err(|source| Error::CreateCString { source })?;
        let _guard = calls().map_err(|_| Error::DlSymUnknown)?;
        unsafe { dlerror() };
        let pointer = unsafe { dlsym(self.handle, name.as_ptr()) };
        let err = unsafe { dlerror() };
        if !err.is_null() {
            return Err(Error::DlSym { desc: unsafe { CStr::from_ptr(err) }.into() });
        }
        // This adapter conservatively rejects null-valued symbols; the loader can
        // resolve them, but null is invalid for some caller-provided T types.
        if pointer.is_null() {
            return Err(Error::DlSymUnknown);
        }
        Ok(Symbol { raw: RawSymbol { pointer, _ty: PhantomData }, _library: PhantomData })
    }

    pub fn close(self) -> Result<(), Error> {
        let _guard = calls().map_err(|_| Error::DlCloseUnknown)?;
        let handle = self.handle;
        mem::forget(self);
        if unsafe { dlclose(handle) } != 0 {
            let err = unsafe { dlerror() };
            return Err(if err.is_null() { Error::DlCloseUnknown }
                else { Error::DlClose { desc: unsafe { CStr::from_ptr(err) }.into() } });
        }
        Ok(())
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let Ok(_guard) = calls() else { return };
        unsafe { dlclose(self.handle) };
    }
}

#[derive(Debug)]
pub struct Symbol<'library, T> {
    raw: RawSymbol<T>,
    _library: PhantomData<&'library Library>,
}

impl<T> Symbol<'_, T> {
    /// # Safety
    /// The caller must keep the library loaded for every use of the raw symbol.
    pub unsafe fn into_raw(self) -> RawSymbol<T> { self.raw }

    /// # Safety
    /// The pointer has no lifetime tying it to its defining library.
    pub unsafe fn try_as_raw_ptr(self) -> Option<*mut c_void> { Some(self.raw.pointer) }
}

impl<T> Deref for Symbol<'_, T> {
    type Target = T;
    fn deref(&self) -> &T { &self.raw }
}

#[derive(Debug)]
pub struct RawSymbol<T> {
    pointer: *mut c_void,
    _ty: PhantomData<T>,
}

impl<T> Deref for RawSymbol<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // get() checked size/alignment; its unsafe contract checks the type.
        unsafe { &*(&self.pointer as *const *mut c_void).cast::<T>() }
    }
}
