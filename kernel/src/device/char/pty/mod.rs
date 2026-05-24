//! Pseudo-terminal pair core.
//!
//! This module provides the architecture-independent PTY data path used by the
//! `/dev/ptmx` and `devpts` layers. It intentionally keeps allocation and
//! namespace policy outside the char-device core so each filesystem open can
//! create an independent master endpoint.

use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, Ordering},
};
use spin::Mutex;

use crate::{
    arch::Trapframe,
    device::{
        Device, DeviceCapability, DeviceType,
        char::{
            CharDevice,
            tty::{TtyBackend, TtyDevice},
        },
    },
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{ReadyInterest, ReadySet, SelectWaitOutcome, Selectable},
    },
    sync::waker::Waker,
};

struct PtyCore {
    master_input: Mutex<VecDeque<u8>>,
    master_waker: Waker,
}

impl PtyCore {
    fn new() -> Self {
        Self {
            master_input: Mutex::new(VecDeque::new()),
            master_waker: Waker::new_interruptible("pty_master_input"),
        }
    }

    fn push_master_output(&self, buffer: &[u8]) -> usize {
        let mut queue = self.master_input.lock();
        for &byte in buffer {
            queue.push_back(byte);
        }
        let len = buffer.len();
        drop(queue);
        if len != 0 {
            self.master_waker.wake_all();
        }
        len
    }

    fn pop_master_input(&self, buffer: &mut [u8]) -> usize {
        let mut queue = self.master_input.lock();
        let mut count = 0;
        while count < buffer.len() {
            let Some(byte) = queue.pop_front() else {
                break;
            };
            buffer[count] = byte;
            count += 1;
        }
        count
    }

    fn master_input_len(&self) -> usize {
        self.master_input.lock().len()
    }
}

struct PtySlaveBackend {
    core: Arc<PtyCore>,
}

impl PtySlaveBackend {
    fn new(core: Arc<PtyCore>) -> Self {
        Self { core }
    }
}

impl TtyBackend for PtySlaveBackend {
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        Ok(self.core.push_master_output(buffer))
    }

    fn can_write(&self) -> bool {
        true
    }
}

/// A Unix98-style PTY pair without devfs registration.
pub struct PtyPair {
    master: Arc<PtyMasterDevice>,
    slave: Arc<TtyDevice>,
    slave_locked: AtomicBool,
}

impl PtyPair {
    /// Create a new pseudo-terminal pair.
    ///
    /// # Arguments
    ///
    /// * `number` - PTY number reserved by the future allocator.
    ///
    /// # Returns
    ///
    /// A master device and slave TTY connected through in-memory queues.
    pub fn new(number: usize) -> Self {
        let core = Arc::new(PtyCore::new());
        let slave_backend = Arc::new(PtySlaveBackend::new(core.clone()));
        let slave = Arc::new(TtyDevice::new_with_backend("pts", slave_backend));
        slave.set_self_ref(Arc::downgrade(&slave));

        let master = Arc::new(PtyMasterDevice {
            number,
            core,
            slave: Arc::downgrade(&slave),
        });

        Self {
            master,
            slave,
            slave_locked: AtomicBool::new(true),
        }
    }

    /// Return the PTY number associated with this pair.
    ///
    /// # Returns
    ///
    /// The allocator-provided PTY number.
    pub fn number(&self) -> usize {
        self.master.number
    }

    /// Return the master endpoint.
    ///
    /// # Returns
    ///
    /// Shared master endpoint for this PTY pair.
    pub fn master(&self) -> Arc<PtyMasterDevice> {
        self.master.clone()
    }

    /// Return the slave TTY endpoint.
    ///
    /// # Returns
    ///
    /// Shared slave TTY endpoint for this PTY pair.
    pub fn slave(&self) -> Arc<TtyDevice> {
        self.slave.clone()
    }

    /// Return whether the slave endpoint is locked.
    ///
    /// # Returns
    ///
    /// `true` when `/dev/pts/N` should reject opens until unlocked.
    pub fn is_slave_locked(&self) -> bool {
        self.slave_locked.load(Ordering::Relaxed)
    }

    /// Set the slave endpoint lock state.
    ///
    /// # Arguments
    ///
    /// * `locked` - New lock state for the slave endpoint.
    pub fn set_slave_locked(&self, locked: bool) {
        self.slave_locked.store(locked, Ordering::Relaxed);
    }
}

/// PTY master endpoint.
///
/// Reads receive bytes written by the slave TTY. Writes inject bytes into the
/// slave line discipline as if they arrived from a terminal emulator.
pub struct PtyMasterDevice {
    number: usize,
    core: Arc<PtyCore>,
    slave: Weak<TtyDevice>,
}

impl PtyMasterDevice {
    /// Return the PTY number for this master.
    ///
    /// # Returns
    ///
    /// The PTY number associated with the connected slave.
    pub fn number(&self) -> usize {
        self.number
    }

    fn slave(&self) -> Result<Arc<TtyDevice>, &'static str> {
        self.slave.upgrade().ok_or("PTY slave is closed")
    }
}

impl Device for PtyMasterDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "ptmx"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }

    fn capabilities(&self) -> &'static [DeviceCapability] {
        &[]
    }
}

impl CharDevice for PtyMasterDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut buffer = [0u8; 1];
        if self.read(&mut buffer) == 1 {
            Some(buffer[0])
        } else {
            None
        }
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.write(&[byte]).map(|_| ())
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        self.core.pop_master_input(buffer)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        let slave = self.slave()?;
        for &byte in buffer {
            slave.inject_input_byte(byte);
        }
        Ok(buffer.len())
    }

    fn can_read(&self) -> bool {
        self.core.master_input_len() != 0
    }

    fn can_write(&self) -> bool {
        self.slave.strong_count() != 0
    }
}

impl Selectable for PtyMasterDevice {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        if interest.read {
            set.read = self.can_read();
        }
        if interest.write {
            set.write = self.can_write();
        }
        set
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        if interest.read && !self.can_read() {
            if let Some(task) = crate::task::mytask() {
                self.core.master_waker.wait(task.get_id(), trapframe);
            }
        }
        SelectWaitOutcome::Ready
    }
}

impl ControlOps for PtyMasterDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Unsupported control command for PTY master")
    }
}

impl MemoryMappingOps for PtyMasterDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by PTY master")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use crate::device::char::{CharDevice, TtyControl};

    use super::*;

    #[test_case]
    fn test_pty_master_write_reaches_slave_ldisc() {
        let pair = PtyPair::new(3);
        pair.slave().set_echo(false);

        pair.master().write(b"hello\n").unwrap();

        let mut buffer = [0u8; 6];
        assert_eq!(pair.slave().read(&mut buffer), 6);
        assert_eq!(&buffer, b"hello\n");
    }

    #[test_case]
    fn test_pty_slave_write_reaches_master() {
        let pair = PtyPair::new(4);

        pair.slave().write(b"out\n").unwrap();

        let mut buffer = [0u8; 5];
        assert_eq!(pair.master().read(&mut buffer), 5);
        assert_eq!(&buffer, b"out\r\n");
    }
}
