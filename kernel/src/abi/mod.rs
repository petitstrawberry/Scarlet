use crate::{arch::Trapframe, task::mytask};
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};
use hashbrown::HashMap;
use spin::Mutex;

pub mod scarlet;

/// ABI module trait.
/// 
/// This trait defines the interface for ABI modules in the Scarlet kernel.
/// ABI modules are responsible for handling system calls and providing
/// the necessary functionality for different application binary interfaces.
/// 
pub trait AbiModule: 'static {
    fn name() -> &'static str
    where
        Self: Sized;

    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str>;
}


pub struct AbiRegistry {
    factories: HashMap<String, fn() -> Box<dyn AbiModule>>,
}

impl AbiRegistry {
    fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    #[allow(static_mut_refs)]
    pub fn global() -> &'static Mutex<AbiRegistry> {
        // スピンロックを使った遅延初期化
        static mut INSTANCE: Option<Mutex<AbiRegistry>> = None;
        static INIT: spin::Once = spin::Once::new();
        
        unsafe {
            INIT.call_once(|| {
                INSTANCE = Some(Mutex::new(AbiRegistry::new()));
            });
            
            // INIT.call_onceが呼ばれた後でのみアクセスするため安全
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
            .insert(T::name().to_string(), || Box::new(T::default()));
    }

    pub fn instantiate(name: &str) -> Option<Box<dyn AbiModule>> {
        let registry = Self::global().lock();
        registry.factories.get(name).map(|f| f())
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
    task.abi.handle_syscall(trapframe)
}