use crate::{arch::Trapframe, task::mytask};
use alloc::{boxed::Box, vec::Vec};

pub mod scarlet;

/// ABI module trait.
/// 
/// This trait defines the interface for ABI modules in the Scarlet kernel.
/// ABI modules are responsible for handling system calls and providing
/// the necessary functionality for different application binary interfaces.
/// 
pub trait AbiModule {
    fn name(&self) -> &'static str;
    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str>;
}

/// ABI registry.
/// 
/// This struct is a singleton that holds a list of registered ABI modules.
/// It provides methods to register new ABI modules and retrieve them by name.
/// 
pub struct AbiRegsitry {
    pub abi: Vec<Box<dyn AbiModule>>,
}

impl AbiRegsitry {
    /// Get the singleton instance of the ABI registry.
    /// 
    /// This method returns a reference to the singleton instance of the ABI registry.
    /// It initializes the instance if it has not been created yet.
    /// 
    /// # Returns
    /// A reference to the singleton instance of the ABI registry.
    /// 
    #[allow(static_mut_refs)]
    pub fn shared() -> &'static Self {
        static mut INSTANCE: Option<AbiRegsitry> = None;
        unsafe {
            if INSTANCE.is_none() {
                INSTANCE = Some(AbiRegsitry::new());
            }
            INSTANCE.as_ref().unwrap()
        }
    }

    fn new() -> Self {
        Self { abi: Vec::new() }
    }

    /// Register a new ABI module.
    ///
    /// This method adds a new ABI module to the registry.
    /// 
    /// # Arguments
    /// * `abi` - A boxed ABI module to be registered.
    /// 
    pub fn register_abi(&mut self, abi: Box<dyn AbiModule>) {
        self.abi.push(abi);
    }

    /// Get an ABI module by name.
    /// 
    /// This method retrieves an ABI module from the registry by its name.
    /// 
    /// # Arguments
    /// * `name` - The name of the ABI module to retrieve.
    /// 
    /// # Returns
    /// An optional reference to the ABI module if found, or `None` if not found.
    /// 
    pub fn get_abi(&self, name: &str) -> Option<&Box<dyn AbiModule>> {
        self.abi.iter().find(|abi| abi.name() == name)
    }
}

pub fn syscall_dispatcher(trapframe: &mut Trapframe) -> Result<usize, &'static str> {
    let task = mytask().unwrap();
    task.abi.handle_syscall(trapframe)
}