//! Load the installed Khronos Vulkan loader through Scarlet's native ELF loader.
//! Keep `Loader` alive until all Vulkan objects created through its entry are destroyed.

use std::ffi::{CStr, c_char, c_int, c_void};

unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(library: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(library: *mut c_void) -> c_int;
    fn dlerror() -> *mut c_char;
}

pub struct Loader {
    entry: ash::Entry,
    library: *mut c_void,
}

fn error() -> String {
    let error = unsafe { dlerror() };
    if error.is_null() {
        "native Vulkan loader operation failed".into()
    } else {
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Loader {
    /// Load the standard native library. Driver discovery belongs to libvulkan,
    /// including its JSON search and VK_DRIVER_FILES override.
    ///
    /// # Safety
    /// The installed library must implement the Vulkan ABI. The caller must keep
    /// this owner alive until all instances, devices and derived entry tables die.
    pub unsafe fn load() -> Result<Self, String> {
        unsafe { Self::load_from(c"/lib/libvulkan.so.1") }
    }

    /// # Safety
    /// Same ownership and ABI requirements as `load`; path names executable code.
    pub unsafe fn load_from(path: &CStr) -> Result<Self, String> {
        // Native scarlet-ld implements immediate binding and global scope only.
        let library = unsafe { dlopen(path.as_ptr(), 0x102) };
        if library.is_null() {
            return Err(error());
        }
        let address = unsafe { dlsym(library, c"vkGetInstanceProcAddr".as_ptr()) };
        if address.is_null() {
            let message = error();
            unsafe {
                dlclose(library);
            }
            return Err(message);
        }
        let get_instance_proc_addr = unsafe { std::mem::transmute(address) };
        let entry = unsafe {
            ash::Entry::from_static_fn(ash::StaticFn {
                get_instance_proc_addr,
            })
        };
        Ok(Self { entry, library })
    }

    pub fn entry(&self) -> &ash::Entry {
        &self.entry
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // scarlet-ld currently releases the handle and pins the object until exit.
        unsafe {
            dlclose(self.library);
        }
    }
}
