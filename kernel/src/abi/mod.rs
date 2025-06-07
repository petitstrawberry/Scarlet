//! ABI module.
//! 
//! This module provides the interface for ABI (Application Binary Interface) modules
//! in the Scarlet kernel. ABI modules are responsible for handling system calls
//! and providing the necessary functionality for different application binary
//! interfaces.
//! 

use crate::{arch::Trapframe, fs::VfsManager, task::mytask};
use alloc::{boxed::Box, string::{String, ToString}, sync::Arc};
use hashbrown::HashMap;
use spin::Mutex;

pub mod scarlet;
pub mod xv6;

pub const MAX_ABI_LENGTH: usize = 64;

/// ABI module trait.
/// 
/// This trait defines the interface for ABI modules in the Scarlet kernel.
/// ABI modules are responsible for handling system calls and providing
/// the necessary functionality for different application binary interfaces.
/// 
/// # Note
/// You must implement the `Default` trait for your ABI module.
/// 
pub trait AbiModule: 'static + Send + Sync {
    fn name() -> &'static str
    where
        Self: Sized;
    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str>;
    fn init(&self) {
        // Default implementation does nothing
    }
    fn init_fs(&self, _vfs: &mut VfsManager) {
        // Default implementation does nothing
    }
}


/// ABI registry.
/// 
/// This struct is responsible for managing the registration and instantiation
/// of ABI modules in the Scarlet kernel.
/// 
pub struct AbiRegistry {
    factories: HashMap<String, fn() -> Arc<dyn AbiModule>>,
}

impl AbiRegistry {
    fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    #[allow(static_mut_refs)]
    pub fn global() -> &'static Mutex<AbiRegistry> {
        // Lazy initialization using spin lock
        static mut INSTANCE: Option<Mutex<AbiRegistry>> = None;
        static INIT: spin::Once = spin::Once::new();
        
        unsafe {
            INIT.call_once(|| {
                INSTANCE = Some(Mutex::new(AbiRegistry::new()));
            });
            
            // Safe to access after INIT.call_once is called
            INSTANCE.as_ref().unwrap()
        }
    }

    pub fn register<T>()
    where
        T: AbiModule + Default + 'static,
    {
        crate::early_println!("Registering ABI module: {}", T::name());
        let mut registry = Self::global().lock();
        registry
            .factories
            .insert(T::name().to_string(), || Arc::new(T::default()));
    }

    pub fn instantiate(name: &str) -> Option<Arc<dyn AbiModule>> {
        let registry = Self::global().lock();
        if let Some(factory) = registry.factories.get(name) {
            let abi = factory();
            abi.init();
            return Some(abi);
        }
        None
    }
}

#[macro_export]
macro_rules! register_abi {
    ($ty:ty) => {
        $crate::abi::AbiRegistry::register::<$ty>();
    };
}

pub fn syscall_dispatcher(trapframe: &mut Trapframe) -> Result<usize, &'static str> {
    let task = mytask().unwrap();
    let abi = task.abi.as_ref().expect("ABI not set");
    abi.handle_syscall(trapframe)
}