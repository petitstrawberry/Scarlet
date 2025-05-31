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
    assert_eq!(header.e_shoff, 3217992, "Unexpected section header offset");
    assert_eq!(header.e_flags, 0x5, "Unexpected flags");
    assert_eq!(header.e_ehsize, 64, "Unexpected ELF header size");
    assert_eq!(header.e_phentsize, 56, "Unexpected program header entry size");
    assert_eq!(header.e_phnum, 4, "Unexpected number of program headers");
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
                assert_eq!(program_header.p_filesz, 0x8888, "Unexpected file size for segment 0");
                assert_eq!(program_header.p_memsz, 0x8888, "Unexpected memory size for segment 0");
                assert_eq!(program_header.p_flags, PF_R | PF_X, "Unexpected flags for segment 0");
                assert_eq!(program_header.p_align, 0x1000, "Unexpected alignment for segment 0");
            }
            1 => {
                assert_eq!(program_header.p_type, PT_LOAD, "Unexpected type for segment 1");
                assert_eq!(program_header.p_offset, 0xa000, "Unexpected offset for segment 1");
                assert_eq!(program_header.p_vaddr, 0x9000, "Unexpected virtual address for segment 1");
                assert_eq!(program_header.p_paddr, 0x9000, "Unexpected physical address for segment 1");
                assert_eq!(program_header.p_filesz, 0x283f, "Unexpected file size for segment 1");
                assert_eq!(program_header.p_memsz, 0x283f, "Unexpected memory size for segment 1");
                assert_eq!(program_header.p_flags, PF_R, "Unexpected flags for segment 1");
                assert_eq!(program_header.p_align, 0x1000, "Unexpected alignment for segment 1");
            }
            2 => {
                assert_eq!(program_header.p_type, PT_LOAD, "Unexpected type for segment 2");
                assert_eq!(program_header.p_offset, 0xd000, "Unexpected offset for segment 2");
                assert_eq!(program_header.p_vaddr, 0xc000, "Unexpected virtual address for segment 2");
                assert_eq!(program_header.p_paddr, 0xc000, "Unexpected physical address for segment 2");
                assert_eq!(program_header.p_filesz, 0x8, "Unexpected file size for segment 2");
                assert_eq!(program_header.p_memsz, 0x2000, "Unexpected memory size for segment 2");
                assert_eq!(program_header.p_flags, PF_R | PF_W, "Unexpected flags for segment 2");
                assert_eq!(program_header.p_align, 0x1000, "Unexpected alignment for segment 2");
            }
            3 => {
                // assert_eq!(program_header.p_type, PT_RISCV_ATTRIBUTES, "Unexpected type for segment 3");
                assert_eq!(program_header.p_offset, 0x1e1f1d, "Unexpected offset for segment 3");
                assert_eq!(program_header.p_filesz, 0x5a, "Unexpected file size for segment 3");
                assert_eq!(program_header.p_memsz, 0x5a, "Unexpected memory size for segment 3");
                assert_eq!(program_header.p_flags, PF_R, "Unexpected flags for segment 3");
                assert_eq!(program_header.p_align, 0x1, "Unexpected alignment for segment 3");
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
    let fs = TestFileSystem::new("test_fs", Box::new(blk_dev), 512);
    let fs_id = manager.register_fs(Box::new(fs));
    manager.mount(fs_id, "/").expect("Failed to mount test filesystem");
    let file_path = "/test.elf";
    manager.create_regular_file(file_path).expect("Failed to create test file");
    let mut file = File::open_with_manager(file_path.to_string(), &mut manager).map_err(|_| "Failed to create file").unwrap();
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

#[test_case]
fn test_load_elf_invalid_magic() {
    use crate::fs::File;
    use crate::task::elf_loader::load_elf_into_task;

    let mut manager = VfsManager::new();
    let blk_dev = MockBlockDevice::new(0, "test_blk", 512, 1024);
    let fs = TestFileSystem::new("test_fs", Box::new(blk_dev), 512);
    let fs_id = manager.register_fs(Box::new(fs));
    manager.mount(fs_id, "/").expect("Failed to mount test filesystem");
    let file_path = "/invalid.elf";
    manager.create_regular_file(file_path).expect("Failed to create test file");

    // Create a mock ELF file with an invalid magic number
    let invalid_elf_data = vec![0u8; 64]; // 64-byte ELF header with all zeros
    let mut file = File::open_with_manager("/invalid.elf".to_string(), &mut manager).unwrap();
    file.write(&invalid_elf_data).expect("Failed to write invalid ELF data");

    // Create a new task
    let mut task = new_user_task("test_invalid_magic".to_string(), 0);

    // Attempt to load the invalid ELF file
    let result = load_elf_into_task(&mut file, &mut task);

    // Assert that the result is an error
    assert!(result.is_err(), "Expected error when loading ELF with invalid magic number");
}

#[test_case]
fn test_load_elf_invalid_alignment() {
    use crate::fs::File;
    use crate::task::elf_loader::load_elf_into_task;

    let mut manager = VfsManager::new();
    let blk_dev = MockBlockDevice::new(0, "test_blk", 512, 1024);
    let fs = TestFileSystem::new( "test_fs", Box::new(blk_dev), 512);
    let fs_id = manager.register_fs(Box::new(fs));
    manager.mount(fs_id, "/").expect("Failed to mount test filesystem");
    let file_path = "/invalid_align.elf";
    manager.create_regular_file(file_path).expect("Failed to create test file");

    // Create a mock ELF file with an invalid alignment
    let mut invalid_elf_data = vec![0u8; 64];
    invalid_elf_data[EI_MAG0] = ELFMAG[0];
    invalid_elf_data[EI_MAG1] = ELFMAG[1];
    invalid_elf_data[EI_MAG2] = ELFMAG[2];
    invalid_elf_data[EI_MAG3] = ELFMAG[3];
    invalid_elf_data[EI_CLASS] = ELFCLASS64;
    invalid_elf_data[EI_DATA] = ELFDATA2LSB;
    invalid_elf_data[16] = 0x2; // e_type
    invalid_elf_data[18] = 0xF3; // e_machine
    invalid_elf_data[20] = 0x1; // e_version
    invalid_elf_data[24] = 0x0; // e_entry
    invalid_elf_data[32] = 0x40; // e_phoff
    invalid_elf_data[54] = 0x38; // e_phentsize
    invalid_elf_data[56] = 0x1; // e_phnum

    // Add a program header with invalid alignment
    invalid_elf_data.extend_from_slice(&[0x1, 0x0, 0x0, 0x0]); // p_type = PT_LOAD
    invalid_elf_data.extend_from_slice(&[0x0, 0x0, 0x0, 0x0]); // p_flags
    invalid_elf_data.extend_from_slice(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]); // p_offset
    invalid_elf_data.extend_from_slice(&[0x1, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]); // p_vaddr (unaligned)
    invalid_elf_data.extend_from_slice(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]); // p_paddr
    invalid_elf_data.extend_from_slice(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]); // p_filesz
    invalid_elf_data.extend_from_slice(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]); // p_memsz
    invalid_elf_data.extend_from_slice(&[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0]); // p_align = 0

    let mut file = File::open_with_manager("/invalid_align.elf".to_string(), &mut manager).map_err(|_| "Failed to create file").unwrap();
    file.write(&invalid_elf_data).expect("Failed to write invalid ELF data");

    // Create a new task
    let mut task = new_user_task("test_invalid_alignment".to_string(), 0);

    // Attempt to load the invalid ELF file
    let result = load_elf_into_task(&mut file, &mut task);

    // Assert that the result is an error
    assert!(result.is_err(), "Expected error when loading ELF with invalid alignment");
}

#[test_case]
fn test_load_elf_bss_zeroed() {
    use crate::fs::File;
    use crate::task::elf_loader::load_elf_into_task;

    let mut manager = VfsManager::new();
    let blk_dev = MockBlockDevice::new(0, "test_blk", 512, 1024);
    let fs = TestFileSystem::new( "test_fs", Box::new(blk_dev), 512);
    let fs_id = manager.register_fs(Box::new(fs));
    manager.mount(fs_id, "/").expect("Failed to mount test filesystem");
    let file_path = "/test_bss.elf";
    manager.create_regular_file(file_path).expect("Failed to create test file");
    let mut file = File::open_with_manager(file_path.to_string(), &mut manager).map_err(|_| "Failed to create file").unwrap();

    // Create a mock ELF file with a .bss section
    let mut elf_data = vec![0u8; 64];
    elf_data[EI_MAG0] = ELFMAG[0];
    elf_data[EI_MAG1] = ELFMAG[1];
    elf_data[EI_MAG2] = ELFMAG[2];
    elf_data[EI_MAG3] = ELFMAG[3];
    elf_data[EI_CLASS] = ELFCLASS64;
    elf_data[EI_DATA] = ELFDATA2LSB;
    elf_data[16] = 0x2; // e_type
    elf_data[18] = 0xF3; // e_machine
    elf_data[20] = 0x1; // e_version
    elf_data[24] = 0x0; // e_entry
    elf_data[32] = 0x40; // e_phoff
    elf_data[54] = 0x38; // e_phentsize
    elf_data[56] = 0x1; // e_phnum

    // Add a program header with .bss section
    elf_data.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // p_type = PT_LOAD (LE)
    elf_data.extend_from_slice(&[0x06, 0x00, 0x00, 0x00]); // p_flags = RW (LE)
    elf_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // p_offset (LE)
    elf_data.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // p_vaddr = 0x1000 (LE)
    elf_data.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // p_paddr = 0x1000 (LE)
    elf_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // p_filesz (LE)
    elf_data.extend_from_slice(&[0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // p_memsz = 0x2000 (LE)
    elf_data.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // p_align = 0x1000 (LE)

    file.write(&elf_data).expect("Failed to write ELF data");

    // Create a new task
    let mut task = new_user_task("test_bss_zeroed".to_string(), 0);

    // Load the ELF file into the task
    load_elf_into_task(&mut file, &mut task).expect("Failed to load ELF file");

    // Verify that the .bss section is zeroed
    let bss_start = 0x1000; // Virtual address of .bss section (aligned to PAGE_SIZE)
    let bss_size = 0x2000; // Size of .bss section (2 * PAGE_SIZE)
    let paddr = task.vm_manager.translate_vaddr(bss_start).expect("Failed to translate .bss start address");

    for i in 0..bss_size {
        let byte: u8;
        unsafe {
            byte = core::ptr::read((paddr + i) as *const u8);
        }
        assert_eq!(byte, 0, "Non-zero byte found in .bss section at offset {}", i);
    }
}

