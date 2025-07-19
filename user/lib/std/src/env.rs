//! Environment variable and command line argument access
//! 
//! This module provides functionality similar to std::env for accessing
//! command line arguments and environment variables in a no_std environment.

use crate::{collections::BTreeMap, string::String, vec::Vec};
use crate::string::ToString;
use core::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);
// Use raw pointers to avoid static mut reference issues
static mut ARGS_PTR: *mut Vec<String> = core::ptr::null_mut();
static mut ENV_MAP_PTR: *mut BTreeMap<String, String> = core::ptr::null_mut();

/// Initialize the environment with command line arguments and environment variables
/// 
/// This should be called from the startup routine with the argc, argv, and envp
/// parameters passed from the kernel.
/// 
/// # Safety
/// 
/// This function is unsafe because it modifies global state and should only be
/// called once during program initialization.
pub unsafe fn init_env(argc: usize, argv: *const *const u8, envp: *const *const u8) {
    if INITIALIZED.load(Ordering::Acquire) {
        return; // Already initialized
    }

    unsafe {
        // Allocate storage for args and env map
        let args_vec = crate::boxed::Box::new(Vec::new());
        let env_map = crate::boxed::Box::new(BTreeMap::new());
        
        ARGS_PTR = crate::boxed::Box::into_raw(args_vec);
        ENV_MAP_PTR = crate::boxed::Box::into_raw(env_map);

        let args = &mut *ARGS_PTR;
        let env_map = &mut *ENV_MAP_PTR;

        // Parse command line arguments
        args.clear();
        
        // Handle NULL argv case
        if !argv.is_null() && argc > 0 {
            for i in 0..argc {
                let arg_ptr = *argv.add(i);
                if !arg_ptr.is_null() {
                    let arg_str = parse_c_string(arg_ptr);
                    args.push(arg_str);
                } else {
                    // If we encounter a NULL pointer in the middle of argv, stop
                    break;
                }
            }
        }

        // Parse environment variables
        env_map.clear();
        
        // Handle NULL envp case
        if !envp.is_null() {
            let mut env_ptr = envp;
            while !(*env_ptr).is_null() {
                let env_str = parse_c_string(*env_ptr);
                if let Some(eq_pos) = env_str.find('=') {
                    let key = env_str[..eq_pos].to_string();
                    let value = env_str[eq_pos + 1..].to_string();
                    env_map.insert(key, value);
                }
                env_ptr = env_ptr.add(1);
            }
        }
        // If envp is NULL, env_map remains empty (which is fine)

        INITIALIZED.store(true, Ordering::Release);
    }
}

/// Parse a null-terminated C string into a Rust String
/// 
/// # Safety
/// 
/// This function assumes ptr is a valid null-terminated C string.
/// Returns an empty string if ptr is null.
unsafe fn parse_c_string(ptr: *const u8) -> String {
    unsafe {
        if ptr.is_null() {
            return String::new();
        }
        
        let mut len = 0;
        const MAX_STRING_LEN: usize = 4096; // Safety limit to prevent infinite loops
        
        while len < MAX_STRING_LEN && *ptr.add(len) != 0 {
            len += 1;
        }
        
        if len == 0 {
            return String::new();
        }
        
        let slice = core::slice::from_raw_parts(ptr, len);
        String::from_utf8_lossy(slice).into_owned()
    }
}

/// Returns an iterator over the command line arguments
/// 
/// The first argument is typically the program name.
/// 
/// # Examples
/// 
/// ```
/// use scarlet_std::env;
/// 
/// for arg in env::args() {
///     println!("Argument: {}", arg);
/// }
/// ```
pub fn args() -> ArgsIterator {
    if !INITIALIZED.load(Ordering::Acquire) {
        panic!("Environment not initialized");
    }
    
    unsafe {
        let args = &*ARGS_PTR;
        ArgsIterator {
            args: args.clone(),
            index: 0,
        }
    }
}

/// Returns a vector of command line arguments
/// 
/// This is similar to std::env::args().collect() but returns the vector directly.
pub fn args_vec() -> Vec<String> {
    if !INITIALIZED.load(Ordering::Acquire) {
        panic!("Environment not initialized");
    }
    
    unsafe {
        let args = &*ARGS_PTR;
        args.clone()
    }
}

/// Fetches the environment variable `key` from the current process
/// 
/// Returns `Some(value)` if the variable is present, `None` otherwise.
/// 
/// # Examples
/// 
/// ```
/// use scarlet_std::env;
/// 
/// let path = env::var("PATH");
/// match path {
///     Some(p) => println!("PATH: {}", p),
///     None => println!("PATH not set"),
/// }
/// ```
pub fn var(key: &str) -> Option<String> {
    if !INITIALIZED.load(Ordering::Acquire) {
        panic!("Environment not initialized");
    }
    
    unsafe {
        let env_map = &*ENV_MAP_PTR;
        env_map.get(key).cloned()
    }
}

/// Returns an iterator over all environment variables
/// 
/// Each item returned by the iterator is a (key, value) pair.
/// 
/// # Examples
/// 
/// ```
/// use scarlet_std::env;
/// 
/// for (key, value) in env::vars() {
///     println!("{}: {}", key, value);
/// }
/// ```
pub fn vars() -> VarsIterator {
    if !INITIALIZED.load(Ordering::Acquire) {
        panic!("Environment not initialized");
    }
    
    unsafe {
        let env_map = &*ENV_MAP_PTR;
        VarsIterator {
            vars: env_map.clone(),
            keys: env_map.keys().cloned().collect(),
            index: 0,
        }
    }
}

/// Sets the environment variable `key` to the value `value` for the current process
/// 
/// # Examples
/// 
/// ```
/// use scarlet_std::env;
/// 
/// env::set_var("MY_VAR", "my_value");
/// assert_eq!(env::var("MY_VAR"), Some("my_value".to_string()));
/// ```
pub fn set_var<K: AsRef<str>, V: AsRef<str>>(key: K, value: V) {
    if !INITIALIZED.load(Ordering::Acquire) {
        panic!("Environment not initialized");
    }
    
    unsafe {
        let env_map = &mut *ENV_MAP_PTR;
        env_map.insert(key.as_ref().to_string(), value.as_ref().to_string());
    }
}

/// Removes an environment variable from the current process
/// 
/// # Examples
/// 
/// ```
/// use scarlet_std::env;
/// 
/// env::set_var("MY_VAR", "my_value");
/// env::remove_var("MY_VAR");
/// assert_eq!(env::var("MY_VAR"), None);
/// ```
pub fn remove_var<K: AsRef<str>>(key: K) {
    if !INITIALIZED.load(Ordering::Acquire) {
        panic!("Environment not initialized");
    }
    
    unsafe {
        let env_map = &mut *ENV_MAP_PTR;
        env_map.remove(key.as_ref());
    }
}

/// Iterator over command line arguments
pub struct ArgsIterator {
    args: Vec<String>,
    index: usize,
}

impl Iterator for ArgsIterator {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.args.len() {
            let arg = self.args[self.index].clone();
            self.index += 1;
            Some(arg)
        } else {
            None
        }
    }
}

/// Iterator over environment variables
pub struct VarsIterator {
    vars: BTreeMap<String, String>,
    keys: Vec<String>,
    index: usize,
}

impl Iterator for VarsIterator {
    type Item = (String, String);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.keys.len() {
            let key = &self.keys[self.index];
            let value = self.vars.get(key).unwrap();
            self.index += 1;
            Some((key.clone(), value.clone()))
        } else {
            None
        }
    }
}
