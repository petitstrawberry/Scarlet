use crate::{platform::NativePlatform, process::Process};
use scarlet_loader_core::{Error, LoaderContext, Machine, ObjectId};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Mutex, MutexGuard, OnceLock};

const RTLD_NOW: c_int = 2;
const RTLD_GLOBAL: c_int = 0x100;

struct Handle {
    object: Option<ObjectId>,
    open: bool,
}

struct Runtime {
    loader: LoaderContext<NativePlatform>,
    handles: Vec<Handle>,
}

// This context belongs to the interpreter, stays resident after the entry
// transfer, and owns both startup DT_NEEDED and later dlopen objects.
static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();

#[derive(Default)]
struct ErrorState {
    pending: Option<CString>,
    returned: Option<CString>,
}

thread_local! {
    static ERROR: RefCell<ErrorState> = RefCell::new(ErrorState::default());
}

fn fail(error: impl std::fmt::Display) {
    let message = error.to_string().replace('\0', "?");
    ERROR.with(|state| state.borrow_mut().pending = Some(CString::new(message).unwrap()));
}

fn lock_runtime() -> Result<MutexGuard<'static, Runtime>, &'static str> {
    RUNTIME
        .get()
        .ok_or("dynamic loader is not initialized")?
        .try_lock()
        .map_err(
            |_| "dynamic loader is busy (constructor reentry and concurrent calls are unsupported)",
        )
}

impl Runtime {
    fn handle(&mut self, object: Option<ObjectId>) -> Result<*mut c_void, Error> {
        let token = self.handles.len().checked_add(1).ok_or(Error::Overflow)?;
        self.handles.push(Handle { object, open: true });
        // Opaque tokens are never dereferenced and are never reused.
        Ok(token as *mut c_void)
    }

    fn scope(&self, handle: *mut c_void) -> Result<Option<ObjectId>, &'static str> {
        (handle as usize)
            .checked_sub(1)
            .and_then(|i| self.handles.get(i))
            .filter(|h| h.open)
            .map(|h| h.object)
            .ok_or("invalid or closed dynamic-library handle")
    }

    unsafe fn initialize_pending(&mut self) -> Result<(), Error> {
        let constructors = self.loader.take_pending_initializers()?;
        for address in constructors {
            // SAFETY: Core validates these as mapped executable addresses.
            // Calling code from the requested ELF is part of dlopen's contract.
            let constructor: unsafe extern "C" fn() = unsafe { std::mem::transmute(address) };
            unsafe {
                constructor();
            }
        }
        Ok(())
    }
}

/// Relocate the kernel-mapped executable and all startup dependencies.
///
/// Returns the original application's entry address. The caller must restore
/// the original stack and argc/argv registers before transferring control.
///
/// # Safety
/// Call once, from `scarlet-ld`, after its own std startup. `initial_stack`
/// must be the persistent kernel initial stack. Main code must not yet run.
pub unsafe fn initialize(initial_stack: usize) -> Result<usize, Error> {
    if RUNTIME.get().is_some() {
        return Err(Error::Unsupported("dynamic loader already initialized"));
    }
    // SAFETY: Forward the kernel initial stack guaranteed by the caller.
    let process = unsafe { Process::from_stack(initial_stack) }?;
    #[cfg(target_arch = "aarch64")]
    let machine = Machine::Aarch64;
    #[cfg(target_arch = "riscv64")]
    let machine = Machine::Riscv64;
    // SAFETY: These ranges describe the kernel-mapped, not-yet-running main.
    let platform =
        unsafe { NativePlatform::new(&process.segments, process.path.clone(), process.bytes) };
    let mut loader = LoaderContext::new(platform, machine);
    for (name, address) in [
        ("dlopen", dlopen as *const () as usize),
        ("dlsym", dlsym as *const () as usize),
        ("dlclose", dlclose as *const () as usize),
        ("dlerror", dlerror as *const () as usize),
    ] {
        loader.add_symbol(name, address);
    }
    // SAFETY: The main is already mapped at the auxv-derived load bias.
    let main = unsafe { loader.load_existing(&process.path, process.bias) }?;
    if loader.entry(main)? != process.entry {
        return Err(Error::Format("loader entry disagrees with AT_ENTRY"));
    }
    RUNTIME
        .set(Mutex::new(Runtime {
            loader,
            handles: Vec::new(),
        }))
        .map_err(|_| Error::Unsupported("dynamic loader initialized concurrently"))?;
    let mut runtime = lock_runtime().map_err(|e| Error::Platform(e.into()))?;
    // SAFETY: Relocation and page permissions have completed for all objects.
    unsafe { runtime.initialize_pending() }?;
    Ok(process.entry)
}

/// Load a DSO into the interpreter's global namespace.
///
/// # Safety
/// A non-null pathname must point to a NUL-terminated string. Loaded code and
/// constructors must obey their ABI and all process memory-safety contracts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void {
    if flags != (RTLD_NOW | RTLD_GLOBAL) {
        fail("only RTLD_NOW | RTLD_GLOBAL is supported");
        return std::ptr::null_mut();
    }
    let result = (|| {
        let mut runtime = lock_runtime().map_err(|e| Error::Platform(e.into()))?;
        let object = if path.is_null() {
            None
        } else {
            // SAFETY: The C caller guarantees a valid terminated pathname.
            let path = unsafe { CStr::from_ptr(path) }
                .to_str()
                .map_err(|_| Error::Unsupported("non-UTF-8 library pathname"))?;
            let object = runtime.loader.load(path)?;
            // SAFETY: dlopen explicitly authorizes execution of this object's
            // initializers after relocation/protection succeeds.
            unsafe { runtime.initialize_pending() }?;
            Some(object)
        };
        runtime.handle(object)
    })();
    match result {
        Ok(handle) => handle,
        Err(error) => {
            fail(format!("{error:?}"));
            std::ptr::null_mut()
        }
    }
}

/// Resolve a handle's dependency scope, or the global scope for NULL.
///
/// # Safety
/// `name` must point to a NUL-terminated string. The caller must use the result
/// with the symbol's actual type and ABI and synchronize mutable-data access.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void {
    if name.is_null() {
        fail("null symbol name");
        return std::ptr::null_mut();
    }
    let result = (|| {
        // SAFETY: The C caller guarantees a valid terminated symbol name.
        let name = unsafe { CStr::from_ptr(name) }
            .to_str()
            .map_err(|_| Error::Unsupported("non-UTF-8 symbol name"))?;
        let runtime = lock_runtime().map_err(|e| Error::Platform(e.into()))?;
        let scope = if handle.is_null() {
            None
        } else {
            runtime
                .scope(handle)
                .map_err(|e| Error::Platform(e.into()))?
        };
        let address = match scope {
            None => runtime.loader.lookup_global(name)?,
            Some(object) => runtime.loader.lookup(object, name)?,
        };
        address.ok_or_else(|| Error::MissingSymbol(name.to_owned()))
    })();
    match result {
        Ok(address) => address as *mut c_void,
        Err(error) => {
            fail(format!("{error:?}"));
            std::ptr::null_mut()
        }
    }
}

/// Release a handle. Objects remain pinned until process exit; finalizers and
/// actual unload are not supported. Stale handles are rejected.
#[unsafe(no_mangle)]
pub extern "C" fn dlclose(handle: *mut c_void) -> c_int {
    let result = (|| {
        let mut runtime = lock_runtime()?;
        runtime.scope(handle)?;
        runtime.handles[handle as usize - 1].open = false;
        Ok::<(), &'static str>(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            fail(error);
            -1
        }
    }
}

/// Return and clear this thread's last pending error. The returned string
/// remains valid until this thread's next call to dlerror.
#[unsafe(no_mangle)]
pub extern "C" fn dlerror() -> *mut c_char {
    ERROR.with(|state| {
        let mut state = state.borrow_mut();
        state.returned = state.pending.take();
        state
            .returned
            .as_ref()
            .map_or(std::ptr::null_mut(), |s| s.as_ptr().cast_mut())
    })
}
