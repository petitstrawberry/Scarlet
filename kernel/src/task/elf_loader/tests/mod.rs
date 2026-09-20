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
        None,
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
    assert!(setup_native_stack(&task, &["bad\0argument"], &[], top, &auxv, None).is_err());
    assert!(
        setup_native_stack(&task, &[&"x".repeat(4 * PAGE_SIZE)], &[], top, &auxv, None).is_err()
    );
    let (sp, _) = setup_native_stack(&task, &[], &["KEY=value"], top, &auxv, None).unwrap();
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
fn relocated_elf_preserves_header_addresses_and_main_program_break() {
    use crate::library::std::usercopy::copy_from_user;
    use scarlet_abi::data_model::AbiDataModel;
    let model = AbiDataModel::NATIVE;
    for headers_in_segment in [false, true] {
        let mut bytes = native_fixture(&vec![0x73; PAGE_SIZE], PAGE_SIZE as u64);
        let (eh, ph_size) = elf_sizes(bytes[EI_CLASS]).unwrap();
        bytes[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        model.write_word(&mut bytes, 24, 0x3000).unwrap();
        let vaddr_offset = if usize::BITS == 32 { 8 } else { 16 };
        model
            .write_word(&mut bytes, eh + vaddr_offset, 0x3000)
            .unwrap();
        if headers_in_segment {
            bytes.copy_within(eh..eh + ph_size, 0x1040);
            let phoff_offset = if usize::BITS == 32 { 28 } else { 32 };
            model.write_word(&mut bytes, phoff_offset, 0x1040).unwrap();
        }
        let manager = VfsManager::new();
        manager.mount(TmpFS::new(0), "/", 0).unwrap();
        manager.create_file("/pie", FileType::RegularFile).unwrap();
        let object = manager.open("/pie", O_RDWR).unwrap();
        let file = object.as_file().unwrap();
        file.write(&bytes).unwrap();
        let task = new_user_task("pie-metadata".into(), 0);
        let result =
            analyze_and_load_elf_with_strategy(file, &task, &LoadStrategy::default()).unwrap();
        assert_eq!(result.base_address, Some(0x10000));
        assert_eq!(result.entry_point, 0x13000);
        assert_eq!(task.brk.load(Ordering::Relaxed), 0x14000);
        if headers_in_segment {
            assert_eq!(result.program_headers.phdr_addr, 0x13040);
        }
        let header = read_elf_header(file).unwrap();
        let mut actual_headers = vec![0; ph_size];
        copy_from_user(
            &task,
            result.program_headers.phdr_addr as usize,
            &mut actual_headers,
        )
        .unwrap();
        let file_offset = header.e_phoff as usize;
        assert_eq!(actual_headers, bytes[file_offset..file_offset + ph_size]);

        let interpreter_task = new_user_task("linker-metadata".into(), 0);
        interpreter_task.brk.store(0x6000, Ordering::Relaxed);
        assert!(
            load_elf_segments_with_base(&header, file, &interpreter_task, 0x20001, false).is_err()
        );
        load_elf_segments_with_base(&header, file, &interpreter_task, 0x20000, false).unwrap();
        assert_eq!(interpreter_task.brk.load(Ordering::Relaxed), 0x6000);
        assert_eq!(
            executable_entry(&header, file, &interpreter_task, 0x20000).unwrap(),
            0x23000
        );
    }
}

// Place one interpreter request in a native fixture without depending on a
// cross-linker. Its PT_LOAD payload still begins at file offset 0x1000.
fn fixture_with_interpreter(path: &str) -> alloc::vec::Vec<u8> {
    use scarlet_abi::data_model::AbiDataModel;
    let model = AbiDataModel::NATIVE;
    let mut bytes = native_fixture(&[0x73; 4], PAGE_SIZE as u64);
    let (eh, ph_size) = elf_sizes(bytes[EI_CLASS]).unwrap();
    let count_offset = if usize::BITS == 32 { 44 } else { 56 };
    bytes[count_offset..count_offset + 2].copy_from_slice(&2u16.to_le_bytes());
    let ph = eh + ph_size;
    bytes[ph..ph + 4].copy_from_slice(&PT_INTERP.to_le_bytes());
    let (file_offset, file_size) = if usize::BITS == 32 { (4, 16) } else { (8, 32) };
    model
        .write_word(&mut bytes, ph + file_offset, 0x200)
        .unwrap();
    model
        .write_word(&mut bytes, ph + file_size, (path.len() + 1) as u64)
        .unwrap();
    bytes[0x200..0x200 + path.len()].copy_from_slice(path.as_bytes());
    bytes
}

#[test_case]
fn native_interpreter_handoff_preserves_main_metadata_and_initial_stack() {
    use crate::library::std::usercopy::copy_from_user;
    use alloc::sync::Arc;
    use scarlet_abi::data_model::AbiDataModel;
    let model = AbiDataModel::NATIVE;
    let word = model.word_width.bytes();
    for interpreter_kind in [ET_EXEC, ET_DYN] {
        let manager = Arc::new(VfsManager::new());
        manager.mount(TmpFS::new(0), "/", 0).unwrap();
        let mut main = fixture_with_interpreter("/scarlet-ld");
        main[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        let mut interpreter = native_fixture(&[0x42; 4], PAGE_SIZE as u64);
        interpreter[16..18].copy_from_slice(&interpreter_kind.to_le_bytes());
        let interpreter_vaddr = if interpreter_kind == ET_EXEC {
            0x4000_0000
        } else {
            0x3000
        };
        let (eh, ph_size) = elf_sizes(main[EI_CLASS]).unwrap();
        let vaddr_offset = if usize::BITS == 32 { 8 } else { 16 };
        model
            .write_word(&mut interpreter, 24, interpreter_vaddr)
            .unwrap();
        model
            .write_word(&mut interpreter, eh + vaddr_offset, interpreter_vaddr)
            .unwrap();
        for (path, bytes) in [("/main", &main), ("/scarlet-ld", &interpreter)] {
            manager.create_file(path, FileType::RegularFile).unwrap();
            manager
                .open(path, O_RDWR)
                .unwrap()
                .as_file()
                .unwrap()
                .write(bytes)
                .unwrap();
        }
        let task = new_user_task("native-interpreter-handoff".into(), 0);
        task.set_vfs(manager.clone());
        let object = manager.open("/main", 0).unwrap();
        let result = analyze_and_load_elf(object.as_file().unwrap(), &task).unwrap();
        assert!(matches!(result.mode, ExecutionMode::Dynamic { .. }));
        assert_eq!(result.original_entry_point, Some(0x11000));
        assert_eq!(result.base_address, Some(0x10000));
        assert_eq!(task.brk.load(Ordering::Relaxed), 0x12000);
        let bias = result.interpreter_base.unwrap();
        if interpreter_kind == ET_EXEC {
            assert_eq!(bias, 0); // AT_BASE is the load bias, not the first mapping.
        } else {
            assert_ne!(bias, 0);
        }
        assert_eq!(result.entry_point, interpreter_vaddr + bias);
        let mut payload = [0; 4];
        copy_from_user(&task, result.entry_point as usize, &mut payload).unwrap();
        assert_eq!(payload, [0x42; 4]);
        let mut headers = vec![0; 2 * ph_size];
        copy_from_user(
            &task,
            result.program_headers.phdr_addr as usize,
            &mut headers,
        )
        .unwrap();
        assert_eq!(headers, main[eh..eh + 2 * ph_size]);

        let top = crate::environment::USER_STACK_END;
        task.allocate_stack_pages(top - PAGE_SIZE, 1).unwrap();
        let auxv = build_auxiliary_vector(&result);
        let (sp, argv) = setup_native_exec_stack(
            &task,
            &object,
            &["display-name", "argument"],
            &["KEY=value"],
            top,
            &result,
        )
        .unwrap();
        assert_eq!(sp % 16, 0);
        assert_eq!(argv, sp + word);
        let mut stack = vec![0; top - sp];
        copy_from_user(&task, sp, &mut stack).unwrap();
        let read = |index| model.read_word(&stack, index * word).unwrap().unsigned();
        assert_eq!(read(0), 2);
        assert_eq!(read(3), 0); // argv terminator
        assert_eq!(read(5), 0); // envp terminator
        for (kind, expected) in [
            (AT_ENTRY, 0x11000),
            (AT_BASE, bias),
            (AT_PHDR, result.program_headers.phdr_addr),
            (AT_PHENT, ph_size as u64),
            (AT_PHNUM, 2),
        ] {
            let index = auxv.iter().position(|entry| entry.a_type == kind).unwrap();
            assert_eq!(read(6 + 2 * index), kind);
            assert_eq!(read(7 + 2 * index), expected);
        }
        let mut fd = None;
        let mut execfn = None;
        let mut index = 6;
        while read(index) != AT_NULL {
            match read(index) {
                AT_EXECFD => fd = Some(read(index + 1) as u32),
                AT_EXECFN => execfn = Some(read(index + 1) as usize),
                _ => {}
            }
            index += 2;
        }
        let fd = fd.expect("dynamic main must supply AT_EXECFD");
        assert!(fd >= 3);
        assert!(!task.handle_table.is_valid_handle(0));
        let held = task.handle_table.get(fd).unwrap();
        let held = held.as_file().unwrap();
        held.seek(SeekFrom::Start(0)).unwrap();
        let mut magic = [0; 4];
        held.read(&mut magic).unwrap();
        assert_eq!(magic, ELFMAG);
        let metadata = task.handle_table.get_metadata(fd).unwrap();
        assert_eq!(
            metadata.access_mode,
            crate::object::handle::AccessMode::ReadOnly
        );
        assert_eq!(
            metadata.special_semantics,
            Some(crate::object::handle::SpecialSemantics::CloseOnExec)
        );
        let offset = execfn.expect("visible main must supply AT_EXECFN") - sp;
        assert_eq!(&stack[offset..offset + 6], b"/main\0");
        for (index, string) in [(1, "display-name"), (2, "argument"), (4, "KEY=value")] {
            let offset = read(index) as usize - sp;
            assert_eq!(&stack[offset..offset + string.len()], string.as_bytes());
            assert_eq!(stack[offset + string.len()], 0);
        }
        // A failed startup stack must roll back its newly inserted handle.
        let before = task.handle_table.open_count();
        assert!(
            setup_native_exec_stack(&task, &object, &["bad\0argument"], &[], top, &result).is_err()
        );
        assert_eq!(task.handle_table.open_count(), before);
        task.handle_table.remove(fd);
        // Source-view files need not be reachable in the target Environment.
        // A different inode at the same spelling must not become AT_EXECFN.
        let other = Arc::new(VfsManager::new());
        other.mount(TmpFS::new(0), "/", 0).unwrap();
        other.create_file("/main", FileType::RegularFile).unwrap();
        other
            .open("/main", O_RDWR)
            .unwrap()
            .as_file()
            .unwrap()
            .write(b"replacement")
            .unwrap();
        task.set_vfs(other);
        assert!(native_executable_path(object.as_file().unwrap(), &task).is_none());
        let (sp, _) =
            setup_native_exec_stack(&task, &object, &["alias"], &[], top, &result).unwrap();
        let mut stack = vec![0; top - sp];
        copy_from_user(&task, sp, &mut stack).unwrap();
        let read = |index| model.read_word(&stack, index * word).unwrap().unsigned();
        let mut index = 4; // argc, one argv, NULL, empty envp NULL
        let mut execfd = None;
        while read(index) != AT_NULL {
            assert_ne!(read(index), AT_EXECFN);
            if read(index) == AT_EXECFD {
                execfd = Some(read(index + 1) as u32);
            }
            index += 2;
        }
        let held = task.handle_table.get(execfd.unwrap()).unwrap();
        let held = held.as_file().unwrap();
        held.seek(SeekFrom::Start(0)).unwrap();
        held.read(&mut magic).unwrap();
        assert_eq!(magic, ELFMAG); // The original image, never the target-view replacement.
    }
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
