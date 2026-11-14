//! Descriptor installation receipts for transactional ABI translations.
use super::*;
use crate::{
    object::{
        KernelObject,
        handle::{HandleMetadata, SpecialSemantics, WeakHandleTable},
    },
    task::Task,
};
use alloc::sync::Weak;

pub(crate) struct DescriptorReceipt {
    pub fd: usize,
    generation: u64,
    table: Weak<IrqRwSpinLock<LinuxFdTable>>,
    handles: WeakHandleTable,
    owner: usize,
    extensions: &'static dyn crate::abi::linux::extensions::LinuxAbiExtensions,
    armed: bool,
}
impl DescriptorReceipt {
    /// Successful ordinary FD delivery transfers ownership to the recipient.
    pub fn release(mut self) {
        self.armed = false;
    }
}
impl Drop for DescriptorReceipt {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(table) = self.table.upgrade() else {
            return;
        };
        let handle = {
            let mut table = table.write();
            if table.generations[self.fd] != self.generation {
                return;
            }
            let Some(handle) = table.fd_to_handle[self.fd].take() else {
                return;
            };
            table.fd_flags[self.fd] = 0;
            table.file_status_flags[self.fd] = 0;
            table.free_fds.push(self.fd);
            handle
        };
        if let Some(handles) = self.handles.upgrade() {
            if let Some(object) = handles.remove(handle) {
                if let Some(task) = crate::sched::scheduler::get_task_by_id(self.owner) {
                    self.extensions.flush_on_close(&task, &object);
                }
                drop(object);
            }
        }
    }
}
impl LinuxAbi {
    /// Install an owned capability without amplifying its access mode. An armed
    /// receipt undoes delivery; generation checks protect concurrently reused fds.
    pub(crate) fn receive_capability(
        &self,
        task: &Task,
        object: KernelObject,
        mut metadata: HandleMetadata,
        status: u32,
    ) -> Result<DescriptorReceipt, usize> {
        let access = match metadata.access_mode {
            crate::object::handle::AccessMode::ReadOnly => 0,
            crate::object::handle::AccessMode::WriteOnly => 1,
            crate::object::handle::AccessMode::ReadWrite => 2,
        };
        metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
        let handle = task
            .handle_table
            .insert_with_metadata(object, metadata)
            .map_err(|_| errno::EMFILE)?;
        let result = (|| {
            let mut table = self.fd_table.write();
            let fd = *table.free_fds.last().ok_or(errno::EMFILE)?;
            let generation = table.generations[fd].checked_add(1).ok_or(errno::EMFILE)?;
            table.free_fds.pop();
            table.generations[fd] = generation;
            table.fd_to_handle[fd] = Some(handle);
            table.fd_flags[fd] = fs::FD_CLOEXEC;
            table.file_status_flags[fd] = (status & !3) | access;
            Ok(DescriptorReceipt {
                fd,
                generation,
                table: Arc::downgrade(&self.fd_table),
                handles: task.handle_table.downgrade(),
                owner: task.get_id(),
                extensions: self.extensions,
                armed: true,
            })
        })();
        if result.is_err() {
            drop(task.handle_table.remove(handle));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn descriptor_delivery_rollback_never_closes_a_reused_fd_and_keeps_access() {
        let mut task = Task::new("fd-transfer".into(), 1, crate::task::TaskType::User);
        task.set_id(60001);
        let mut abi = LinuxAbi::default();
        let memory = Arc::new(crate::ipc::SharedMemory::new(4096, 1).unwrap());
        let receipt = abi
            .receive_capability(
                &task,
                KernelObject::SharedMemory(memory.clone()),
                HandleMetadata {
                    access_mode: crate::object::handle::AccessMode::ReadOnly,
                    ..HandleMetadata::default()
                },
                2 | 0x800,
            )
            .unwrap();
        let fd = receipt.fd;
        assert_eq!(abi.get_fd_flags(fd), Some(fs::FD_CLOEXEC));
        assert_eq!(abi.get_file_status_flags(fd), Some(0x800));
        let handle = abi.remove_fd(fd).unwrap();
        drop(task.handle_table.remove(handle));
        let replacement = task
            .handle_table
            .insert(KernelObject::SharedMemory(memory))
            .unwrap();
        abi.allocate_specific_fd(fd, replacement).unwrap();
        drop(receipt);
        assert_eq!(abi.get_handle(fd), Some(replacement));
        let fresh = abi
            .receive_capability(
                &task,
                KernelObject::SharedMemory(Arc::new(
                    crate::ipc::SharedMemory::new(4096, 1).unwrap(),
                )),
                HandleMetadata::default(),
                0,
            )
            .unwrap();
        let fd = fresh.fd;
        drop(fresh);
        assert!(abi.get_handle(fd).is_none());
    }
    #[test_case]
    fn descriptor_receipt_is_weak_and_ordinary_delivery_releases_cleanup() {
        let mut task = Task::new("fd-delivery".into(), 1, crate::task::TaskType::User);
        task.set_id(60002);
        let abi = LinuxAbi::default();
        let table_refs = Arc::strong_count(&abi.fd_table);
        let receipt = abi
            .receive_capability(
                &task,
                KernelObject::SharedMemory(Arc::new(
                    crate::ipc::SharedMemory::new(4096, 1).unwrap(),
                )),
                HandleMetadata::default(),
                2,
            )
            .unwrap();
        let fd = receipt.fd;
        assert_eq!(Arc::strong_count(&abi.fd_table), table_refs);
        receipt.release();
        assert!(abi.get_handle(fd).is_some());
    }
}
