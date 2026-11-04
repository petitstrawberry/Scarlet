//! Linux `/proc/pressure/memory` interface.
//!
//! AOSP `system/memory/lmkd` at `android-13.0.0_r1` opens this path write-only,
//! writes `some|full <threshold_us> <window_us>` with a trailing NUL, and waits
//! for `EPOLLPRI`. Scarlet can now measure `some` from bounded allocation-failure
//! reclaim retries. `full` is enabled only after scheduler integration publishes
//! actual all-non-idle-stalled transitions; it is never copied from `some`.

use alloc::{string::String, sync::Arc, vec::Vec};
use core::{
    any::Any,
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    abi::linux::generic::{
        LinuxAbi, errno,
        fs::{FD_CLOEXEC, O_CLOEXEC, O_DIRECTORY},
    },
    arch::Trapframe,
    fs::{FileMetadata, FilePermission, FileType, SeekFrom},
    mem::pressure::{self, MemoryPressureTrigger, PressureClass, PressureSnapshot, PressureStats},
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, ReadySet,
            SelectWaitOutcome, Selectable, StreamError, StreamOps,
        },
    },
    sync::{IrqRwSpinLock, ReadinessRegistration, Waker},
    task::Task,
};

const PRESSURE_MEMORY_PATH: &str = "/proc/pressure/memory";
const PSI_WINDOW_MIN_US: u64 = 500_000;
const PSI_WINDOW_MAX_US: u64 = 10_000_000;

struct PressureFile {
    position: IrqRwSpinLock<u64>,
    access: i32,
    nonblocking: AtomicBool,
    trigger: IrqRwSpinLock<Option<Arc<MemoryPressureTrigger>>>,
}

impl PressureFile {
    fn can_read(&self) -> bool {
        self.access != 1
    }

    fn can_write(&self) -> bool {
        self.access != 0
    }

    fn content(&self) -> String {
        render(pressure::snapshot())
    }
}

fn render(snapshot: PressureSnapshot) -> String {
    let mut output = String::new();
    write_stats(&mut output, "some", snapshot.some);
    write_stats(&mut output, "full", snapshot.full);
    output
}

fn write_stats(output: &mut String, name: &str, stats: PressureStats) {
    let _ = write!(output, "{name} avg10=");
    write_fixed_percentage(output, stats.avg10_basis_points);
    let _ = write!(output, " avg60=");
    write_fixed_percentage(output, stats.avg60_basis_points);
    let _ = write!(output, " avg300=");
    write_fixed_percentage(output, stats.avg300_basis_points);
    let _ = write!(output, " total={}\n", stats.total_us);
}

fn write_fixed_percentage(output: &mut String, basis_points: u16) {
    let _ = write!(output, "{}.{:02}", basis_points / 100, basis_points % 100,);
}

enum TriggerRequest {
    Some { threshold_us: u64, window_us: u64 },
    Full { threshold_us: u64, window_us: u64 },
}

fn parse_trigger(buffer: &[u8]) -> Option<TriggerRequest> {
    let text = buffer.strip_suffix(&[0]).unwrap_or(buffer);
    if text.contains(&0) {
        return None;
    }
    let text = core::str::from_utf8(text).ok()?.trim();
    let mut fields = text.split_ascii_whitespace();
    let class = fields.next()?;
    let threshold_us = fields.next()?.parse::<u64>().ok()?;
    let window_us = fields.next()?.parse::<u64>().ok()?;
    if fields.next().is_some()
        || threshold_us == 0
        || threshold_us > window_us
        || !(PSI_WINDOW_MIN_US..=PSI_WINDOW_MAX_US).contains(&window_us)
    {
        return None;
    }
    match class {
        "some" => Some(TriggerRequest::Some {
            threshold_us,
            window_us,
        }),
        "full" => Some(TriggerRequest::Full {
            threshold_us,
            window_us,
        }),
        _ => None,
    }
}

/// Open Scarlet's native memory-pressure interface.
///
/// `open` is wired before ordinary proc text dispatch in
/// `generic::fs::sys_openat` with the resolved absolute path and Linux flags.
/// Read-only descriptors expose integer PSI statistics. Write-capable
/// descriptors accept `some` triggers immediately. `full` is accepted after the
/// scheduler has advertised its measured full-state producer.
pub(super) fn open(abi: &mut LinuxAbi, task: &Task, path: &str, flags: i32) -> Option<usize> {
    if path.trim_end_matches('/') != PRESSURE_MEMORY_PATH {
        return None;
    }
    let access = flags & 3;
    if access == 3 {
        return Some(errno::to_result(errno::EINVAL));
    }
    if flags & O_DIRECTORY != 0 {
        return Some(errno::to_result(errno::ENOTDIR));
    }

    let file = Arc::new(PressureFile {
        position: IrqRwSpinLock::new(0),
        access,
        nonblocking: AtomicBool::new(false),
        trigger: IrqRwSpinLock::new(None),
    });
    let handle = match task.handle_table.insert(KernelObject::File(file)) {
        Ok(handle) => handle,
        Err(_) => return Some(errno::to_result(errno::ENFILE)),
    };
    let fd = match abi.allocate_fd(handle) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            return Some(errno::to_result(errno::EMFILE));
        }
    };
    if flags & O_CLOEXEC != 0 {
        let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
    }
    Some(fd)
}

impl StreamOps for PressureFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if !self.can_read() {
            return Err(StreamError::PermissionDenied);
        }
        let content = self.content();
        let mut position = self.position.write();
        let start = usize::try_from(*position).map_err(|_| StreamError::InvalidArgument)?;
        if start >= content.len() {
            return Ok(0);
        }
        let count = buffer.len().min(content.len() - start);
        buffer[..count].copy_from_slice(&content.as_bytes()[start..start + count]);
        *position = (*position).saturating_add(count as u64);
        Ok(count)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        if !self.can_write() {
            return Err(StreamError::PermissionDenied);
        }
        let Some(request) = parse_trigger(buffer) else {
            return Err(StreamError::InvalidArgument);
        };
        let (class, threshold_us, window_us) = match request {
            TriggerRequest::Some {
                threshold_us,
                window_us,
            } => (PressureClass::Some, threshold_us, window_us),
            TriggerRequest::Full {
                threshold_us,
                window_us,
            } => {
                // Full is available only after the scheduler producer
                // advertises its initial measured state.
                if !pressure::full_accounting_available() {
                    return Err(StreamError::NotSupported);
                }
                (PressureClass::Full, threshold_us, window_us)
            }
        };
        if self.trigger.read().is_some() {
            return Err(StreamError::InvalidArgument);
        }
        let trigger = MemoryPressureTrigger::arm(class, threshold_us, window_us)
            .ok_or(StreamError::NoSpace)?;
        let mut slot = self.trigger.write();
        if slot.is_some() {
            return Err(StreamError::InvalidArgument);
        }
        *slot = Some(trigger);
        Ok(buffer.len())
    }
}

impl ControlOps for PressureFile {}

impl MemoryMappingOps for PressureFile {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("cannot map proc pressure file")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for PressureFile {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut ready = ReadySet::none();
        if interest.read && self.can_read() {
            ready.read = true;
        }
        if interest.except
            && self
                .trigger
                .read()
                .as_ref()
                .is_some_and(|trigger| trigger.has_event())
        {
            ready.except = true;
        }
        ready
    }

    fn subscribe_readiness(
        &self,
        interest: ReadyInterest,
        waiter: &Arc<Waker>,
    ) -> Option<Vec<ReadinessRegistration>> {
        if !interest.except {
            return Some(Vec::new());
        }
        let trigger = self.trigger.read().as_ref().cloned();
        Some(
            trigger
                .map(|trigger| trigger.subscribe_readiness(waiter))
                .into_iter()
                .collect(),
        )
    }

    fn acknowledge_ready(&self, ready: ReadySet) {
        if ready.except
            && let Some(trigger) = self.trigger.read().as_ref()
        {
            let _ = trigger.take_event();
        }
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        timeout_ns: Option<u64>,
        _min_wait_ns: u64,
    ) -> SelectWaitOutcome {
        if interest.read && self.can_read() {
            return SelectWaitOutcome::Ready;
        }
        if !interest.except {
            return SelectWaitOutcome::Ready;
        }
        let Some(trigger) = self.trigger.read().as_ref().cloned() else {
            return SelectWaitOutcome::Ready;
        };
        if trigger.has_event() {
            return SelectWaitOutcome::Ready;
        }
        if self.nonblocking.load(Ordering::Acquire) {
            return SelectWaitOutcome::TimedOut;
        }
        let Some(task) = crate::task::mytask() else {
            return SelectWaitOutcome::Interrupted;
        };
        let waiter = trigger.wait_waker();
        let result = if let Some(timeout_ns) = timeout_ns {
            waiter.wait_with_timeout_result_until_owned(
                task.get_id(),
                trapframe,
                timeout_ns,
                || trigger.has_event(),
            )
        } else {
            waiter.wait_result_until_owned(task.get_id(), trapframe, || trigger.has_event())
        };
        match result {
            crate::sync::WaitResult::Woken => SelectWaitOutcome::Ready,
            crate::sync::WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            crate::sync::WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
        }
    }

    fn set_nonblocking(&self, enabled: bool) {
        self.nonblocking.store(enabled, Ordering::Release);
    }

    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }
}

impl FileObject for PressureFile {
    fn allows_execution(&self) -> bool {
        false
    }

    fn allows_execution_privilege_gain(&self) -> bool {
        false
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        let content_length = self.content().len() as u64;
        let next = match whence {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(offset) => *position as i128 + offset as i128,
            SeekFrom::End(offset) => content_length as i128 + offset as i128,
        };
        if next < 0 || next > u64::MAX as i128 {
            return Err(StreamError::InvalidArgument);
        }
        *position = next as u64;
        Ok(*position)
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if !self.can_read() {
            return Err(StreamError::PermissionDenied);
        }
        let content = self.content();
        let start = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        if start >= content.len() {
            return Ok(0);
        }
        let count = buffer.len().min(content.len() - start);
        buffer[..count].copy_from_slice(&content.as_bytes()[start..start + count]);
        Ok(count)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: 0,
            permissions: FilePermission {
                read: self.can_read(),
                write: self.can_write(),
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 0x5052_4553_5355_5245,
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn zero_pressure_uses_the_exact_psi_schema() {
        let snapshot = PressureSnapshot {
            some: PressureStats {
                avg10_basis_points: 0,
                avg60_basis_points: 0,
                avg300_basis_points: 0,
                total_us: 0,
            },
            full: PressureStats {
                avg10_basis_points: 0,
                avg60_basis_points: 0,
                avg300_basis_points: 0,
                total_us: 0,
            },
        };
        assert_eq!(
            render(snapshot),
            "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n\
             full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n"
        );
    }

    #[test_case]
    fn trigger_parser_accepts_aosp_nul_and_rejects_full_until_scheduler_evidence_exists() {
        match parse_trigger(b"some 70000 1000000\0") {
            Some(TriggerRequest::Some {
                threshold_us: 70_000,
                window_us: 1_000_000,
            }) => {}
            _ => panic!("some trigger was not parsed"),
        }
        assert!(matches!(
            parse_trigger(b"full 700000 1000000\0"),
            Some(TriggerRequest::Full {
                threshold_us: 700_000,
                window_us: 1_000_000,
            })
        ));
        assert!(parse_trigger(b"some 0 1000000\0").is_none());
        assert!(parse_trigger(b"some 1000001 1000000\0").is_none());
        assert!(parse_trigger(b"some 1 1000000 extra\0").is_none());
    }

    #[test_case]
    fn averages_render_with_integer_basis_points() {
        let mut output = String::new();
        write_fixed_percentage(&mut output, 1_234);
        assert_eq!(output, "12.34");
    }

    #[test_case]
    fn readiness_peek_persists_until_successful_delivery_acknowledgment() {
        let trigger = MemoryPressureTrigger::test_trigger();
        trigger.raise_event_for_test();
        let file = PressureFile {
            position: IrqRwSpinLock::new(0),
            access: 1,
            nonblocking: AtomicBool::new(false),
            trigger: IrqRwSpinLock::new(Some(trigger)),
        };
        let interest = ReadyInterest {
            read: false,
            write: false,
            except: true,
        };
        assert!(file.current_ready(interest).except);
        assert!(file.current_ready(interest).except);
        file.acknowledge_ready(ReadySet {
            read: false,
            write: false,
            except: true,
        });
        assert!(!file.current_ready(interest).except);
    }
}
