//! ELF loader test suite.
//!
//! Tests for ELF binary loading and execution, including integration with VFS manager for filesystem-based executable loading in isolated namespaces.

use crate::fs::{FileType, SeekFrom, VfsManager, drivers::tmpfs::TmpFS};
use crate::task::new_user_task;

use super::*;

const O_RDWR: u32 = 0x2;

#[test_case]
fn auxiliary_vectors_use_the_selected_abi_width() {
    use scarlet_abi::data_model::{AbiDataModel, ByteOrder, WordWidth};
    let model32 = AbiDataModel::new(WordWidth::Bits32, ByteOrder::Little);
    let model64 = AbiDataModel::new(WordWidth::Bits64, ByteOrder::Little);
    let entries = [AuxVec::new(AT_ENTRY, 0x1234_5678), AuxVec::new(AT_NULL, 0)];
    let bytes = encode_auxiliary_vector(&entries, model32).unwrap();
    assert_eq!(bytes.len(), 16);
    assert_eq!(
        model32.read_word(&bytes, 4).unwrap().unsigned(),
        0x1234_5678
    );
    assert_eq!(model32.read_word(&bytes, 8).unwrap().unsigned(), AT_NULL);
    assert_eq!(
        encode_auxiliary_vector(&entries, model64).unwrap().len(),
        32
    );
    let wide = [AuxVec::new(AT_PHDR, 0x1_1234_5678)];
    assert!(encode_auxiliary_vector(&wide, model32).is_err());
    assert_eq!(
        model64
            .read_word(&encode_auxiliary_vector(&wide, model64).unwrap(), 8)
            .unwrap()
            .unsigned(),
        0x1_1234_5678
    );
}

#[test_case]
fn native_stack_preserves_auxv_and_strings_across_pages() {
    use crate::library::std::usercopy::copy_from_user;
    use scarlet_abi::data_model::AbiDataModel;
    let model = AbiDataModel::NATIVE;
    let word = model.word_width.bytes();
    let task = new_user_task("native-stack".into(), 0);
    let top = crate::environment::USER_STACK_END;
    task.allocate_stack_pages(top - 4 * PAGE_SIZE, 4).unwrap();
    let long_argument = "a".repeat(PAGE_SIZE + 17);
    let auxv = [AuxVec::new(AT_ENTRY, 0x1000), AuxVec::new(AT_NULL, 0)];
    let (sp, argv) = setup_native_stack(
        &task,
        &["program", &long_argument],
        &["KEY=value"],
        top,
        &auxv,
    )
    .unwrap();
    assert_eq!(sp % 16, 0);
    assert_eq!(argv, sp + word);
    let mut bytes = vec![0; top - sp];
    copy_from_user(&task, sp, &mut bytes).unwrap();
    let read = |index| model.read_word(&bytes, index * word).unwrap().unsigned() as usize;
    assert_eq!(read(0), 2);
    assert_eq!(read(3), 0); // argv terminator
    assert_eq!(read(5), 0); // envp terminator, immediately followed by auxv
    assert_eq!(read(6), AT_ENTRY as usize);
    assert_eq!(read(7), 0x1000);
    assert_eq!(read(8), AT_NULL as usize);
    for (index, string) in [
        (1, "program"),
        (2, long_argument.as_str()),
        (4, "KEY=value"),
    ] {
        let offset = read(index) - sp;
        assert_eq!(&bytes[offset..offset + string.len()], string.as_bytes());
        assert_eq!(bytes[offset + string.len()], 0);
    }
    assert!(setup_native_stack(&task, &["bad\0argument"], &[], top, &auxv).is_err());
    assert!(setup_native_stack(&task, &[&"x".repeat(4 * PAGE_SIZE)], &[], top, &auxv).is_err());
    let (sp, _) = setup_native_stack(&task, &[], &["KEY=value"], top, &auxv).unwrap();
    let mut words = vec![0; 4 * word];
    copy_from_user(&task, sp, &mut words).unwrap();
    assert_eq!(model.read_word(&words, 0).unwrap().unsigned(), 0);
    assert_eq!(model.read_word(&words, word).unwrap().unsigned(), 0);
    assert!(model.read_word(&words, 2 * word).unwrap().unsigned() != 0);
}

// Construct actual class-specific headers for mapped-memory tests. The payload
// is inspected as data; these fixtures are never executed by a CPU.
fn fixture(class: u8, machine: u16, payload: &[u8], mem_size: u64) -> alloc::vec::Vec<u8> {
    let (eh, ph) = elf_sizes(class).unwrap();
    let mut bytes = vec![0; 0x1000 + payload.len()];
    bytes[..4].copy_from_slice(&ELFMAG);
    bytes[4] = class;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    let tail;
    if class == ELFCLASS32 {
        bytes[24..28].copy_from_slice(&0x1000u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&(eh as u32).to_le_bytes());
        tail = 36;
        for (offset, value) in [
            (0, PT_LOAD),
            (4, 0x1000),
            (8, 0x1000),
            (12, 0),
            (16, payload.len() as u32),
            (20, mem_size as u32),
            (24, PF_R | PF_W | PF_X),
            (28, 0x1000),
        ] {
            bytes[eh + offset..eh + offset + 4].copy_from_slice(&value.to_le_bytes());
        }
    } else {
        bytes[24..32].copy_from_slice(&0x1000u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&(eh as u64).to_le_bytes());
        tail = 48;
        bytes[eh..eh + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        bytes[eh + 4..eh + 8].copy_from_slice(&(PF_R | PF_W | PF_X).to_le_bytes());
        for (offset, value) in [
            (8, 0x1000),
            (16, 0x1000),
            (24, 0),
            (32, payload.len() as u64),
            (40, mem_size),
            (48, 0x1000),
        ] {
            bytes[eh + offset..eh + offset + 8].copy_from_slice(&value.to_le_bytes());
        }
    }
    bytes[tail + 4..tail + 6].copy_from_slice(&(eh as u16).to_le_bytes());
    bytes[tail + 6..tail + 8].copy_from_slice(&(ph as u16).to_le_bytes());
    bytes[tail + 8..tail + 10].copy_from_slice(&1u16.to_le_bytes());
    bytes[0x1000..].copy_from_slice(payload);
    bytes
}

fn native_fixture(payload: &[u8], mem_size: u64) -> alloc::vec::Vec<u8> {
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    let machine = 243;
    #[cfg(target_arch = "aarch64")]
    let machine = 183;
    fixture(
        if usize::BITS == 32 {
            ELFCLASS32
        } else {
            ELFCLASS64
        },
        machine,
        payload,
        mem_size,
    )
}

#[test_case]
fn elf32_fields_and_program_header_order_are_decoded_independently_of_host_width() {
    let mut bytes = fixture(ELFCLASS32, 243, &[1, 2, 3, 4], 0x2000);
    bytes[24..28].copy_from_slice(&0xf123_4567u32.to_le_bytes());
    let header = ElfHeader::parse(&bytes[..52]).unwrap();
    assert_eq!(header.e_entry, 0xf123_4567);
    assert_eq!(header.e_phoff, 52);
    let ph = ProgramHeader::parse(&bytes[52..84], header.ei_class, true).unwrap();
    assert_eq!(ph.p_flags, PF_R | PF_W | PF_X);
    assert_eq!(ph.p_offset, 0x1000);
    assert_eq!(ph.p_filesz, 4);
    assert_eq!(ph.p_memsz, 0x2000);
    assert!(ElfHeader::parse(&bytes[..51]).is_err());
    assert!(ProgramHeader::parse(&bytes[52..83], ELFCLASS32, true).is_err());
    bytes[5] = 0;
    assert!(ElfHeader::parse(&bytes).is_err());
}

#[test_case]
fn execution_rejects_wrong_machine_and_pointer_width() {
    let native = native_fixture(&[0; 4], 4);
    let mut header = ElfHeader::parse(&native).unwrap();
    assert!(header.validate_executable().is_ok());
    header.e_machine = 0;
    assert!(header.validate_executable().is_err());
    let mut header = ElfHeader::parse(&native).unwrap();
    header.ei_class = if usize::BITS == 32 {
        ELFCLASS64
    } else {
        ELFCLASS32
    };
    assert!(header.validate_executable().is_err());
}

#[test_case]
fn test_parse_elf_header() {
    let elf_data: &[u8] = include_bytes!("test.elf");
    // Attempt to parse the ELF
    let result = ElfHeader::parse(&elf_data);
    // Check if the ELF header is valid
    assert!(
        result.is_ok(),
        "Failed to parse ELF header: {:?}",
        result.err()
    );
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
    assert_eq!(
        header.e_phentsize, 56,
        "Unexpected program header entry size"
    );
    assert_eq!(header.e_phnum, 4, "Unexpected number of program headers");
    assert_eq!(
        header.e_shentsize, 64,
        "Unexpected section header entry size"
    );
    assert_eq!(header.e_shnum, 19, "Unexpected number of section headers");
    assert_eq!(
        header.e_shstrndx, 17,
        "Unexpected section header string table index"
    );
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
        let program_header =
            ProgramHeader::parse(ph_buffer, header.ei_class, header.ei_data == ELFDATA2LSB)
                .expect("Failed to parse program header");

        match i {
            0 => {
                assert_eq!(
                    program_header.p_type, PT_LOAD,
                    "Unexpected type for segment 0"
                );
                assert_eq!(
                    program_header.p_offset, 0x1000,
                    "Unexpected offset for segment 0"
                );
                assert_eq!(
                    program_header.p_vaddr, 0x0,
                    "Unexpected virtual address for segment 0"
                );
                assert_eq!(
                    program_header.p_paddr, 0x0,
                    "Unexpected physical address for segment 0"
                );
                assert_eq!(
                    program_header.p_filesz, 0x8888,
                    "Unexpected file size for segment 0"
                );
                assert_eq!(
                    program_header.p_memsz, 0x8888,
                    "Unexpected memory size for segment 0"
                );
                assert_eq!(
                    program_header.p_flags,
                    PF_R | PF_X,
                    "Unexpected flags for segment 0"
                );
                assert_eq!(
                    program_header.p_align, 0x1000,
                    "Unexpected alignment for segment 0"
                );
            }
            1 => {
                assert_eq!(
                    program_header.p_type, PT_LOAD,
                    "Unexpected type for segment 1"
                );
                assert_eq!(
                    program_header.p_offset, 0xa000,
                    "Unexpected offset for segment 1"
                );
                assert_eq!(
                    program_header.p_vaddr, 0x9000,
                    "Unexpected virtual address for segment 1"
                );
                assert_eq!(
                    program_header.p_paddr, 0x9000,
                    "Unexpected physical address for segment 1"
                );
                assert_eq!(
                    program_header.p_filesz, 0x283f,
                    "Unexpected file size for segment 1"
                );
                assert_eq!(
                    program_header.p_memsz, 0x283f,
                    "Unexpected memory size for segment 1"
                );
                assert_eq!(
                    program_header.p_flags, PF_R,
                    "Unexpected flags for segment 1"
                );
                assert_eq!(
                    program_header.p_align, 0x1000,
                    "Unexpected alignment for segment 1"
                );
            }
            2 => {
                assert_eq!(
                    program_header.p_type, PT_LOAD,
                    "Unexpected type for segment 2"
                );
                assert_eq!(
                    program_header.p_offset, 0xd000,
                    "Unexpected offset for segment 2"
                );
                assert_eq!(
                    program_header.p_vaddr, 0xc000,
                    "Unexpected virtual address for segment 2"
                );
                assert_eq!(
                    program_header.p_paddr, 0xc000,
                    "Unexpected physical address for segment 2"
                );
                assert_eq!(
                    program_header.p_filesz, 0x8,
                    "Unexpected file size for segment 2"
                );
                assert_eq!(
                    program_header.p_memsz, 0x2000,
                    "Unexpected memory size for segment 2"
                );
                assert_eq!(
                    program_header.p_flags,
                    PF_R | PF_W,
                    "Unexpected flags for segment 2"
                );
                assert_eq!(
                    program_header.p_align, 0x1000,
                    "Unexpected alignment for segment 2"
                );
            }
            3 => {
                // assert_eq!(program_header.p_type, PT_RISCV_ATTRIBUTES, "Unexpected type for segment 3");
                assert_eq!(
                    program_header.p_offset, 0x1e1f1d,
                    "Unexpected offset for segment 3"
                );
                assert_eq!(
                    program_header.p_filesz, 0x5a,
                    "Unexpected file size for segment 3"
                );
                assert_eq!(
                    program_header.p_memsz, 0x5a,
                    "Unexpected memory size for segment 3"
                );
                assert_eq!(
                    program_header.p_flags, PF_R,
                    "Unexpected flags for segment 3"
                );
                assert_eq!(
                    program_header.p_align, 0x1,
                    "Unexpected alignment for segment 3"
                );
            }
            _ => panic!("Unexpected program header index: {}", i),
        }
    }
}

#[test_case]
fn test_load_elf() {
    use crate::task::elf_loader::load_elf_into_task;

    let manager = VfsManager::new();
    let fs = TmpFS::new(0);
    manager
        .mount(fs.clone(), "/", 0)
        .expect("Failed to mount test filesystem");
    let file_path = "/test.elf";
    manager
        .create_file(file_path, FileType::RegularFile)
        .expect("Failed to create test file");
    let kernel_obj = manager.open(file_path, 0).expect("Failed to open file");
    let file = kernel_obj.as_file().expect("Failed to get file reference");
    file.write(&native_fixture(&0x73u32.to_le_bytes(), 0x1000))
        .expect("Failed to write test ELF file");

    // Seek to beginning for reading
    file.seek(SeekFrom::Start(0))
        .expect("Failed to seek to start");

    // Create a new task
    let mut task = new_user_task("test".to_string(), 0);

    // Load the ELF file into the task
    let entry_point = load_elf_into_task(file, &mut task).expect("Failed to load ELF file");

    let kaddr = task
        .vm_manager
        .translate_to_kva(entry_point as usize)
        .expect(
            format!(
                "Failed to translate entry point address: {:#x}",
                entry_point
            )
            .as_str(),
        );

    // Read the instruction at the entry point
    let instruction: u32;
    unsafe {
        instruction = core::ptr::read(kaddr as *const u32);
    }

    // Expected instruction at the entry point (e.g., a jump instruction)
    let expected_instruction: u32 = 0x00000073; // Example: ecall instruction

    // Assert that the instruction matches the expected value
    assert_eq!(
        instruction, expected_instruction,
        "Entry point instruction does not match expected value"
    );
}

#[test_case]
fn test_load_elf_invalid_magic() {
    use crate::task::elf_loader::load_elf_into_task;

    let manager = VfsManager::new();
    let fs = TmpFS::new(0);
    manager
        .mount(fs.clone(), "/", 0)
        .expect("Failed to mount test filesystem");
    let file_path = "/invalid.elf";
    manager
        .create_file(file_path, FileType::RegularFile)
        .expect("Failed to create test file");

    // Create a mock ELF file with an invalid magic number
    let invalid_elf_data = vec![0u8; 64]; // 64-byte ELF header with all zeros
    let kernel_obj = manager.open("/invalid.elf", 0).unwrap();
    let file = kernel_obj.as_file().expect("Failed to get file reference");
    file.write(&invalid_elf_data)
        .expect("Failed to write invalid ELF data");
    file.seek(SeekFrom::Start(0))
        .expect("Failed to seek to start");

    // Create a new task
    let mut task = new_user_task("test_invalid_magic".to_string(), 0);

    // Attempt to load the invalid ELF file
    let result = load_elf_into_task(file, &mut task);

    // Assert that the result is an error
    assert!(
        result.is_err(),
        "Expected error when loading ELF with invalid magic number"
    );
}

#[test_case]
fn test_load_elf_invalid_alignment() {
    use crate::task::elf_loader::load_elf_into_task;

    let manager = VfsManager::new();
    let fs = TmpFS::new(0);
    manager
        .mount(fs.clone(), "/", 0)
        .expect("Failed to mount test filesystem");
    let file_path = "/invalid_align.elf";
    manager
        .create_file(file_path, FileType::RegularFile)
        .expect("Failed to create test file");

    let mut invalid_elf_data = native_fixture(&[0; 4], 0x1000);
    let (eh, _) = elf_sizes(invalid_elf_data[4]).unwrap();
    let offset = if invalid_elf_data[4] == ELFCLASS32 {
        eh + 28
    } else {
        eh + 48
    };
    invalid_elf_data[offset..offset + 4].copy_from_slice(&3u32.to_le_bytes());

    let kernel_obj = manager
        .open("/invalid_align.elf", O_RDWR)
        .expect("Failed to open test ELF file");
    let file = kernel_obj.as_file().expect("Failed to get file reference");
    file.write(&invalid_elf_data)
        .expect("Failed to write invalid ELF data");

    // Create a new task
    let mut task = new_user_task("test_invalid_alignment".to_string(), 0);

    // Attempt to load the invalid ELF file
    let result = load_elf_into_task(file, &mut task);

    // Assert that the result is an error
    assert!(
        result.is_err(),
        "Expected error when loading ELF with invalid alignment"
    );
}

#[test_case]
fn test_load_elf_bss_zeroed() {
    use crate::task::elf_loader::load_elf_into_task;

    let manager = VfsManager::new();
    let fs = TmpFS::new(0);
    manager
        .mount(fs.clone(), "/", 0)
        .expect("Failed to mount test filesystem");
    let file_path = "/test_bss.elf";
    manager
        .create_file(file_path, FileType::RegularFile)
        .expect("Failed to create test file");
    let kernel_obj = manager
        .open(file_path, O_RDWR)
        .expect("Failed to open test ELF file");
    let file = kernel_obj.as_file().expect("Failed to get file reference");

    let elf_data = native_fixture(&[], 0x2000);

    file.write(&elf_data).expect("Failed to write ELF data");

    // Create a new task
    let mut task = new_user_task("test_bss_zeroed".to_string(), 0);

    // Load the ELF file into the task
    load_elf_into_task(file, &mut task).expect("Failed to load ELF file");

    // Verify that the .bss section is zeroed
    let bss_start = 0x1000; // Virtual address of .bss section (aligned to PAGE_SIZE)
    let bss_size = 0x2000; // Size of .bss section (2 * PAGE_SIZE)
    let kaddr = task
        .vm_manager
        .translate_to_kva(bss_start)
        .expect("Failed to translate .bss start address");

    for i in 0..bss_size {
        let byte: u8;
        unsafe {
            byte = core::ptr::read((kaddr + i) as *const u8);
        }
        assert_eq!(
            byte, 0,
            "Non-zero byte found in .bss section at offset {}",
            i
        );
    }
}
