use alloc::boxed::Box;

use crate::{device::block::mockblk::MockBlockDevice, fs::{testfs::TestFileSystem, VfsManager}, task::new_user_task};

use super::*;

#[test_case]
fn test_parse_elf_header() {
    let elf_data: &[u8] = include_bytes!("test.elf");
    // Attempt to parse the ELF
    let result = ElfHeader::parse(&elf_data);
    // Check if the ELF header is valid
    assert!(result.is_ok(), "Failed to parse ELF header: {:?}", result.err());
    let header = result.unwrap();

    // Verify the parsed ELF header matches the expected values
    assert_eq!(header.ei_class, ELFCLASS64, "Unexpected ELF class");
    assert_eq!(header.ei_data, ELFDATA2LSB, "Unexpected ELF data encoding");
    assert_eq!(header.e_type, 0x2, "Unexpected ELF type");
    assert_eq!(header.e_machine, 0xF3, "Unexpected machine type");
    assert_eq!(header.e_version, 0x1, "Unexpected ELF version");
    assert_eq!(header.e_entry, 0x0, "Unexpected entry point address");
    assert_eq!(header.e_phoff, 64, "Unexpected program header offset");
    assert_eq!(header.e_shoff, 3209672, "Unexpected section header offset");
    assert_eq!(header.e_flags, 0x5, "Unexpected flags");
    assert_eq!(header.e_ehsize, 64, "Unexpected ELF header size");
    assert_eq!(header.e_phentsize, 56, "Unexpected program header entry size");
    assert_eq!(header.e_phnum, 5, "Unexpected number of program headers");
    assert_eq!(header.e_shentsize, 64, "Unexpected section header entry size");
    assert_eq!(header.e_shnum, 19, "Unexpected number of section headers");
    assert_eq!(header.e_shstrndx, 17, "Unexpected section header string table index");
}

#[test_case]
fn test_parse_program_headers() {
    let elf_data: &[u8] = include_bytes!("test.elf");
    // Attempt to parse the ELF header
    let header = ElfHeader::parse(&elf_data).expect("Failed to parse ELF header");

    // Iterate through program headers and validate them
    for i in 0..header.e_phnum {
        let offset = header.e_phoff + (i as u64) * (header.e_phentsize as u64);
        let ph_buffer = &elf_data[offset as usize..(offset + header.e_phentsize as u64) as usize];
        let program_header = ProgramHeader::parse(ph_buffer, header.ei_data == ELFDATA2LSB)
            .expect("Failed to parse program header");

        match i {
            0 => {
                assert_eq!(program_header.p_type, PT_LOAD, "Unexpected type for segment 0");
                assert_eq!(program_header.p_offset, 0x1000, "Unexpected offset for segment 0");
                assert_eq!(program_header.p_vaddr, 0x0, "Unexpected virtual address for segment 0");
                assert_eq!(program_header.p_paddr, 0x0, "Unexpected physical address for segment 0");
                assert_eq!(program_header.p_filesz, 0x78f4, "Unexpected file size for segment 0");
                assert_eq!(program_header.p_memsz, 0x78f4, "Unexpected memory size for segment 0");
                assert_eq!(program_header.p_flags, PF_R | PF_X, "Unexpected flags for segment 0");
                assert_eq!(program_header.p_align, 0x1000, "Unexpected alignment for segment 0");
            }
            1 => {
                assert_eq!(program_header.p_type, PT_LOAD, "Unexpected type for segment 1");
                assert_eq!(program_header.p_offset, 0x88f8, "Unexpected offset for segment 1");
                assert_eq!(program_header.p_vaddr, 0x78f8, "Unexpected virtual address for segment 1");
                assert_eq!(program_header.p_paddr, 0x78f8, "Unexpected physical address for segment 1");
                assert_eq!(program_header.p_filesz, 0x27b0, "Unexpected file size for segment 1");
                assert_eq!(program_header.p_memsz, 0x27b0, "Unexpected memory size for segment 1");
                assert_eq!(program_header.p_flags, PF_R, "Unexpected flags for segment 1");
                assert_eq!(program_header.p_align, 0x1000, "Unexpected alignment for segment 1");
            }
            2 => {
                assert_eq!(program_header.p_type, PT_LOAD, "Unexpected type for segment 2");
                assert_eq!(program_header.p_offset, 0xb0a8, "Unexpected offset for segment 2");
                assert_eq!(program_header.p_vaddr, 0xa0a8, "Unexpected virtual address for segment 2");
                assert_eq!(program_header.p_paddr, 0xa0a8, "Unexpected physical address for segment 2");
                assert_eq!(program_header.p_filesz, 0x0, "Unexpected file size for segment 2");
                assert_eq!(program_header.p_memsz, 0x1f58, "Unexpected memory size for segment 2");
                assert_eq!(program_header.p_flags, PF_R | PF_W, "Unexpected flags for segment 2");
                assert_eq!(program_header.p_align, 0x1000, "Unexpected alignment for segment 2");
            }
            3 => {
                // assert_eq!(program_header.p_type, PT_GNU_STACK, "Unexpected type for segment 3");
                assert_eq!(program_header.p_flags, PF_R | PF_W, "Unexpected flags for segment 3");
            }
            4 => {
                // assert_eq!(program_header.p_type, PT_RISCV_ATTRIBUTES, "Unexpected type for segment 4");
                assert_eq!(program_header.p_offset, 0x1dfe42, "Unexpected offset for segment 4");
                assert_eq!(program_header.p_filesz, 0x5a, "Unexpected file size for segment 4");
                assert_eq!(program_header.p_memsz, 0x5a, "Unexpected memory size for segment 4");
                assert_eq!(program_header.p_flags, PF_R, "Unexpected flags for segment 4");
                assert_eq!(program_header.p_align, 0x1, "Unexpected alignment for segment 4");
            }
            _ => panic!("Unexpected program header index: {}", i),
        }
    }
}

#[test_case]
fn test_load_elf() {
    use crate::fs::File;
    use crate::task::elf_loader::load_elf_into_task;

    let mut manager = VfsManager::new();
    let blk_dev = MockBlockDevice::new(0, "test_blk", 512, 1024);
    let fs = TestFileSystem::new(0, "test_fs", Box::new(blk_dev), 512);
    let fs_id = manager.register_fs(Box::new(fs));
    manager.mount(fs_id, "/").expect("Failed to mount test filesystem");
    let file_path = "/test.elf";
    manager.create_file(file_path).expect("Failed to create test file");
    let mut file = File::with_manager(file_path.to_string(), &mut manager);
    file.open(0).expect("Failed to open test file");
    file.write(include_bytes!("test.elf")).expect("Failed to write test ELF file");
    
    // Create a new task
    let mut task = new_user_task("test".to_string(), 0);
    
    // Load the ELF file into the task
    let entry_point = load_elf_into_task(&mut file, &mut task).expect("Failed to load ELF file");
    
    // Translate the entry point virtual address to a physical address
    let paddr = task.vm_manager.translate_vaddr(entry_point as usize).expect(format!("Failed to translate entry point address: {:#x}", entry_point).as_str());

    // Read the instruction at the entry point
    let instruction: u32;
    unsafe {
        instruction = core::ptr::read(paddr as *const u32);
    }

    // Expected instruction at the entry point (e.g., a jump instruction)
    let expected_instruction: u32 = 0x00000073; // Example: ecall instruction

    // Assert that the instruction matches the expected value
    assert_eq!(instruction, expected_instruction, "Entry point instruction does not match expected value");
}

