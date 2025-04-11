use alloc::boxed::Box;

use crate::{device::block::mockblk::MockBlockDevice, fs::{testfs::TestFileSystem, VfsManager}, println, print, task::new_user_task};

use super::*;

#[test_case]
fn test_load_embedded_elf() {
    let mut manager = VfsManager::new();
    // Include the ELF binary
    let elf_data: &[u8] = include_bytes!("test.elf");
    let blk_dev = MockBlockDevice::new(0, "blk", 512, 1);
    let fs = TestFileSystem::new(0, "testfs", Box::new(blk_dev), 512);
    let fs_id = manager.register_fs(Box::new(fs));
    manager.mount(fs_id, "/").unwrap();
    manager.create_file("/init").unwrap();
    let mut file = File::with_manager("/init".to_string(), &mut manager);
    file.open(0).unwrap();
    file.write(elf_data).unwrap();

    let mut task = new_user_task("init".to_string(), 0);

    // Attempt to load the ELF into the task
    let result = load_elf_into_task(&mut file, &mut task);

    // Assert that the ELF was loaded successfully
    assert!(result.is_ok(), "Failed to load embedded ELF: {:?}", result.err());

    // Optionally, verify the entry point or other properties
    let entry_point = result.unwrap();

    let paddr = task.vm_manager.translate_vaddr(entry_point as usize);
    assert!(paddr.is_some(), "Failed to translate entry point virtual address to physical address");
    let paddr = paddr.unwrap();
    let mut buffer = [0u8; 4];
    unsafe {
        core::ptr::copy_nonoverlapping(paddr as *const u8, buffer.as_mut_ptr(), 4);
    }

    let entry_point_value = u32::from_le_bytes(buffer);
    assert_eq!(entry_point_value, 0x00000073);
}
