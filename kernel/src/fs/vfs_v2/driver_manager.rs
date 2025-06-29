//! VFS v2 Driver Manager
//!
//! v2用のファイルシステムドライバ管理・生成の仕組み。
//! - ドライバはID(enum)で登録・生成
//! - 柔軟な生成API（option string, params, memory, block device等）
//! - v1の設計を参考にしたtraitベース

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::RwLock;

use super::core::FileSystemOperations;
use crate::fs::params::FileSystemParams;

/// v2用ファイルシステムID
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileSystemId {
    TmpFS,
    CpioFS,
    OverlayFS,
    // 必要に応じて追加
}

/// v2用ファイルシステムドライバtrait
pub trait FileSystemDriverV2: Send + Sync {
    fn id(&self) -> FileSystemId;
    fn name(&self) -> &'static str;
    fn create_from_option_string(&self, option: Option<&str>) -> Arc<dyn FileSystemOperations>;
    fn create_from_params(&self, params: &dyn FileSystemParams) -> Arc<dyn FileSystemOperations>;
    // 必要に応じて他の生成APIも追加可能
}

/// v2用ドライバマネージャ
pub struct FileSystemDriverManagerV2 {
    drivers: RwLock<BTreeMap<FileSystemId, Arc<dyn FileSystemDriverV2>>>,
}

impl FileSystemDriverManagerV2 {
    pub fn new() -> Self {
        Self { drivers: RwLock::new(BTreeMap::new()) }
    }
    pub fn register_driver(&self, driver: Arc<dyn FileSystemDriverV2>) {
        self.drivers.write().insert(driver.id(), driver);
    }
    pub fn get_driver(&self, id: FileSystemId) -> Option<Arc<dyn FileSystemDriverV2>> {
        self.drivers.read().get(&id).cloned()
    }
    pub fn create_from_option_string(&self, id: FileSystemId, option: Option<&str>) -> Option<Arc<dyn FileSystemOperations>> {
        self.get_driver(id).map(|drv| drv.create_from_option_string(option))
    }
    pub fn create_from_params(&self, id: FileSystemId, params: &dyn FileSystemParams) -> Option<Arc<dyn FileSystemOperations>> {
        self.get_driver(id).map(|drv| drv.create_from_params(params))
    }
}

// グローバルなv2ドライバマネージャ（unsafeでstatic化も可）
use core::sync::atomic::{AtomicPtr, Ordering};
static mut V2_DRIVER_MANAGER: Option<FileSystemDriverManagerV2> = None;

pub fn get_v2_driver_manager() -> &'static FileSystemDriverManagerV2 {
    unsafe {
        if V2_DRIVER_MANAGER.is_none() {
            V2_DRIVER_MANAGER = Some(FileSystemDriverManagerV2::new());
        }
        V2_DRIVER_MANAGER.as_ref().unwrap()
    }
}
