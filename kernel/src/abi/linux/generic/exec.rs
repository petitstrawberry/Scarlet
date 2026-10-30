//! Linux process-entry data which needs storage in the new user stack.

use crate::executor::executor::{
    MAX_EXEC_STRING_LENGTH, exec_argument_bytes, exec_string_storage_bytes,
};
use crate::library::std::string::{StringConversionError, parse_c_string_from_userspace};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::task::Task;
use crate::task::elf_loader::{AT_CLKTCK, AT_NULL, AT_RANDOM, AuxVec};
use alloc::{string::String, vec::Vec};

/// Read an exec vector with the same storage limit as the transactional loader.
/// argv and envp share a byte budget, including pointer slots and terminating
/// NULs. Each string separately obeys Linux's MAX_ARG_STRLEN, not PATH_MAX.
pub(crate) fn parse_exec_strings(
    task: &Task,
    address: usize,
    remaining: &mut usize,
) -> Result<Vec<String>, StringConversionError> {
    let mut strings = Vec::new();
    if address == 0 {
        return Ok(strings);
    }
    loop {
        let offset = strings
            .len()
            .checked_mul(core::mem::size_of::<usize>())
            .and_then(|offset| address.checked_add(offset))
            .ok_or(StringConversionError::TranslationError)?;
        let mut pointer = [0u8; core::mem::size_of::<usize>()];
        copy_from_user(task, offset, &mut pointer)
            .map_err(|_| StringConversionError::TranslationError)?;
        let pointer = usize::from_ne_bytes(pointer);
        if pointer == 0 {
            return Ok(strings);
        }
        let pointer_bytes = core::mem::size_of::<usize>();
        if *remaining <= pointer_bytes {
            return Err(StringConversionError::ExceedsMaxLength);
        }
        let length_limit = (*remaining - pointer_bytes).min(MAX_EXEC_STRING_LENGTH);
        let string = parse_c_string_from_userspace(task, pointer, length_limit)?;
        *remaining -= exec_string_storage_bytes(string.len())
            .ok_or(StringConversionError::ExceedsMaxLength)?;
        strings.push(string);
    }
}

/// Keep ordinary stacks small, but leave a full ordinary stack below large
/// argv/envp data. The extra page covers auxv, alignment and loader scratch.
pub(crate) fn setup_exec_stack(
    task: &Task,
    argv: &[&str],
    envp: &[&str],
) -> Result<(usize, usize), &'static str> {
    let bytes = exec_argument_bytes(argv, envp).ok_or("argument list too large")?;
    let minimum = if bytes + crate::environment::PAGE_SIZE <= crate::vm::DEFAULT_USER_STACK_BYTES {
        crate::vm::DEFAULT_USER_STACK_BYTES
    } else {
        bytes + crate::vm::DEFAULT_USER_STACK_BYTES + crate::environment::PAGE_SIZE
    };
    crate::vm::setup_user_stack_with_size(task, minimum)
}

/// A 16-byte auxv pair can straddle noncontiguous physical stack pages.
pub(crate) fn write_auxiliary_vector(
    task: &Task,
    address: usize,
    auxv: &[AuxVec],
) -> Result<(), &'static str> {
    for (index, entry) in auxv.iter().enumerate() {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&entry.a_type.to_ne_bytes());
        bytes[8..].copy_from_slice(&entry.a_val.to_ne_bytes());
        let address = index
            .checked_mul(16)
            .and_then(|offset| address.checked_add(offset))
            .ok_or("Auxiliary vector address overflow")?;
        copy_to_user(task, address, &bytes).map_err(|_| "Cannot populate auxiliary vector")?;
    }
    Ok(())
}

pub(crate) fn conversion_errno(error: StringConversionError) -> usize {
    match error {
        StringConversionError::ExceedsMaxLength | StringConversionError::TooManyStrings => {
            super::errno::E2BIG
        }
        _ => super::errno::EFAULT,
    }
}

/// Place a string vector in ascending address order on the descending stack.
/// Linux applications may rewrite argv in place, treating its strings as one
/// contiguous region (Box64 does this when removing its own argv[0]).
pub(crate) fn push_string_vector(
    task: &Task,
    sp: &mut usize,
    strings: &[&str],
) -> Result<Vec<u64>, &'static str> {
    let mut cursor = *sp;
    let mut addresses = Vec::with_capacity(strings.len());
    for string in strings.iter().rev() {
        let length = string.len().checked_add(1).ok_or("Exec string too long")?;
        cursor = cursor.checked_sub(length).ok_or("Exec stack underflow")?;
        // A string can cross physically noncontiguous stack pages.
        copy_to_user(task, cursor, string.as_bytes()).map_err(|_| "Cannot populate exec string")?;
        copy_to_user(task, cursor + string.len(), &[0])
            .map_err(|_| "Cannot terminate exec string")?;
        addresses.push(cursor as u64);
    }
    addresses.reverse();
    *sp = cursor;
    Ok(addresses)
}

/// Publish the Linux accounting unit used by Bionic's sysconf(_SC_CLK_TCK).
/// This is an ABI conversion unit, independent of the hardware timer frequency.
pub(crate) fn add_clock_auxv(auxv: &mut Vec<AuxVec>) -> Result<(), &'static str> {
    let end = auxv.iter().position(|entry| entry.a_type == AT_NULL)
        .ok_or("Missing auxiliary vector terminator")?;
    if let Some(entry) = auxv[..end].iter_mut().find(|entry| entry.a_type == AT_CLKTCK) {
        entry.a_val = super::time::USER_HZ;
    } else {
        auxv.insert(end, AuxVec::new(AT_CLKTCK, super::time::USER_HZ));
    }
    Ok(())
}

/// Supply the sixteen bytes required by glibc's stack and pointer guards.
/// `task` is the unpublished exec image, which may differ from the current task.
pub(crate) fn add_random_auxv(
    task: &Task,
    sp: &mut usize,
    auxv: &mut Vec<AuxVec>,
) -> Result<(), &'static str> {
    let address = sp.checked_sub(16).ok_or("Stack underflow for AT_RANDOM")? & !15;
    let terminator = auxv
        .iter()
        .position(|entry| entry.a_type == AT_NULL)
        .ok_or("Missing auxiliary vector terminator")?;
    let mut random = [0u8; 16];
    let filled = crate::random::RandomManager::get_random_bytes(&mut random);
    // Match Scarlet's existing getrandom policy when no entropy source exists.
    crate::random::fill_fallback_random(&mut random[filled..]);
    copy_to_user(task, address, &random).map_err(|_| "Cannot populate AT_RANDOM")?;
    auxv.insert(terminator, AuxVec::new(AT_RANDOM, address as u64));
    *sp = address;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{PAGE_SIZE, USER_STACK_END};
    use crate::library::std::usercopy::copy_from_user;
    use crate::task::TaskType;

    fn string_vector(task: &Task, sp: &mut usize, strings: &[&str]) -> usize {
        let addresses = push_string_vector(task, sp, strings).unwrap();
        let mut pointers =
            Vec::with_capacity((addresses.len() + 1) * core::mem::size_of::<usize>());
        for address in addresses {
            pointers.extend_from_slice(&(address as usize).to_ne_bytes());
        }
        pointers.extend_from_slice(&0usize.to_ne_bytes());
        *sp = (*sp - pointers.len()) & !15;
        copy_to_user(task, *sp, &pointers).unwrap();
        *sp
    }

    #[test_case]
    fn linux_exec_accepts_python_postinst_argument_count() {
        use crate::executor::executor::MAX_EXEC_STRING_BYTES;
        let task = Task::new("python-postinst-argv".into(), 0, TaskType::User);
        task.allocate_stack_pages(USER_STACK_END - 8 * PAGE_SIZE, 8)
            .unwrap();
        let mut sp = USER_STACK_END;
        let mut args = alloc::vec!["/usr/lib/python3.13/collections/__init__.py"; 289];
        args[..4].copy_from_slice(&[
            "/usr/bin/python3.13",
            "-E",
            "-S",
            "/usr/lib/python3.13/py_compile.py",
        ]);
        let env = ["DEBIAN_FRONTEND=noninteractive", "PATH=/usr/bin:/bin"];
        let argv = string_vector(&task, &mut sp, &args);
        let envp = string_vector(&task, &mut sp, &env);
        let mut remaining = MAX_EXEC_STRING_BYTES;
        assert_eq!(
            parse_exec_strings(&task, argv, &mut remaining).unwrap(),
            args
        );
        assert_eq!(
            parse_exec_strings(&task, envp, &mut remaining).unwrap(),
            env
        );
        assert_eq!(
            MAX_EXEC_STRING_BYTES - remaining,
            exec_argument_bytes(&args, &env).unwrap()
        );
    }

    #[test_case]
    fn linux_exec_limits_include_pointers_and_share_environment_budget() {
        use crate::executor::executor::MAX_EXEC_STRING_BYTES;
        let largest = "x".repeat(MAX_EXEC_STRING_LENGTH - 1);
        let unit = exec_string_storage_bytes(largest.len()).unwrap();
        let mut args = alloc::vec![largest.as_str(); 48];
        let final_bytes = MAX_EXEC_STRING_BYTES - 47 * unit;
        args[47] = &largest[..final_bytes - 1 - core::mem::size_of::<usize>()];
        assert_eq!(exec_argument_bytes(&args, &[]), Some(MAX_EXEC_STRING_BYTES));
        assert_eq!(exec_argument_bytes(&args, &[""]), None);
        assert_eq!(
            exec_argument_bytes(&alloc::vec![""; 4096], &[]),
            Some(4096 * (1 + core::mem::size_of::<usize>()))
        );
        assert_eq!(exec_string_storage_bytes(MAX_EXEC_STRING_LENGTH), None);
        assert_eq!(exec_string_storage_bytes(usize::MAX), None);

        let task = Task::new("exec-byte-budget".into(), 0, TaskType::User);
        task.allocate_stack_pages(USER_STACK_END - PAGE_SIZE, 1)
            .unwrap();
        let mut sp = USER_STACK_END;
        let argv = string_vector(&task, &mut sp, &["a"]);
        let envp = string_vector(&task, &mut sp, &[""]);
        let mut remaining = 2 + core::mem::size_of::<usize>();
        assert_eq!(
            parse_exec_strings(&task, argv, &mut remaining).unwrap(),
            ["a"]
        );
        assert_eq!(remaining, 0);
        assert_eq!(
            parse_exec_strings(&task, envp, &mut remaining),
            Err(StringConversionError::ExceedsMaxLength)
        );
        assert!(
            parse_exec_strings(&task, 0, &mut remaining)
                .unwrap()
                .is_empty()
        );
        let mut remaining = 2 + core::mem::size_of::<usize>();
        assert_eq!(
            parse_exec_strings(&task, usize::MAX, &mut remaining),
            Err(StringConversionError::TranslationError)
        );
    }

    #[test_case]
    fn linux_exec_per_string_limit_and_large_stack_storage() {
        use crate::executor::executor::MAX_EXEC_STRING_BYTES;
        let task = Task::new("linux-large-argv".into(), 0, TaskType::User);
        let string = "x".repeat(MAX_EXEC_STRING_LENGTH - 1);
        let args = alloc::vec![string.as_str(); 48];
        let mut accepted = args;
        accepted[47] = "";
        let storage = exec_argument_bytes(&accepted, &[]).unwrap();
        let (base, top) = setup_exec_stack(&task, &accepted, &[]).unwrap();
        assert!(top - base >= storage + crate::vm::DEFAULT_USER_STACK_BYTES);
        let mut sp = top;
        let argv = string_vector(&task, &mut sp, &[string.as_str()]);
        let mut remaining = MAX_EXEC_STRING_BYTES;
        assert_eq!(
            parse_exec_strings(&task, argv, &mut remaining).unwrap(),
            [string.as_str()]
        );
        let too_long = "x".repeat(MAX_EXEC_STRING_LENGTH);
        let argv = string_vector(&task, &mut sp, &[too_long.as_str()]);
        let mut remaining = MAX_EXEC_STRING_BYTES;
        assert_eq!(
            parse_exec_strings(&task, argv, &mut remaining),
            Err(StringConversionError::ExceedsMaxLength)
        );
        assert_eq!(remaining, MAX_EXEC_STRING_BYTES);
    }

    #[test_case]
    fn linux_exec_auxv_pair_crosses_noncontiguous_pages() {
        use crate::mem::page::ContiguousPages;
        use crate::vm::vmem::{
            MemoryArea, PhysicalMemoryArea, VirtualMemoryMap, VirtualMemoryPermission,
        };
        let pages = ContiguousPages::new(3).unwrap();
        let task = Task::new("linux-auxv-pages".into(), 0, TaskType::User);
        let base = 0x20000;
        for (virtual_page, physical_page) in [(0, 0), (1, 2)] {
            let va = base + virtual_page * PAGE_SIZE;
            let pa = pages.as_paddr() + (physical_page * PAGE_SIZE) as u64;
            task.vm_manager
                .add_memory_map(VirtualMemoryMap::new(
                    PhysicalMemoryArea::new(pa, pa + PAGE_SIZE as u64 - 1),
                    MemoryArea::new(va, va + PAGE_SIZE - 1),
                    VirtualMemoryPermission::Read as usize
                        | VirtualMemoryPermission::Write as usize
                        | VirtualMemoryPermission::User as usize,
                    false,
                    None,
                ))
                .unwrap();
        }
        let auxv = [AuxVec::new(AT_RANDOM, 0x12345678), AuxVec::new(AT_NULL, 0)];
        let address = base + PAGE_SIZE - 8;
        write_auxiliary_vector(&task, address, &auxv).unwrap();
        let mut bytes = [0; 32];
        copy_from_user(&task, address, &mut bytes).unwrap();
        assert_eq!(&bytes[..8], &AT_RANDOM.to_ne_bytes());
        assert_eq!(&bytes[8..16], &0x12345678u64.to_ne_bytes());
        assert_eq!(&bytes[16..], &[0; 16]);
        let skipped = unsafe {
            core::slice::from_raw_parts(pages.as_ptr().cast::<u8>().add(PAGE_SIZE), PAGE_SIZE)
        };
        assert!(skipped.iter().all(|b| *b == 0));
    }

    #[test_case]
    fn linux_exec_strings_support_box64_argv_compaction() {
        let task = Task::new("linux-argv".into(), 0, TaskType::User);
        task.allocate_stack_pages(USER_STACK_END - PAGE_SIZE, 1)
            .unwrap();
        let mut sp = USER_STACK_END;
        let env = push_string_vector(&task, &mut sp, &["A=1", "B=2"]).unwrap();
        let args = push_string_vector(
            &task,
            &mut sp,
            &["/usr/local/bin/box64", "/usr/lib/wine/wine64", "--version"],
        )
        .unwrap();
        let expected = b"/usr/local/bin/box64\0/usr/lib/wine/wine64\0--version\0A=1\0B=2\0";
        let mut bytes = alloc::vec![0; expected.len()];
        copy_from_user(&task, sp, &mut bytes).unwrap();
        assert_eq!(bytes.as_slice(), expected);
        assert_eq!(args[0] as usize, sp);
        assert_eq!(args[1] - args[0], 21);
        assert_eq!(args[2] - args[1], 21);
        assert_eq!(env[0] - args[2], 10);
        assert_eq!(env[1] - env[0], 4);

        // Box64 slides argv[1..] over argv[0], then clears the vacated tail.
        let diff = args[1].checked_sub(args[0]).unwrap() as usize;
        let end = (env[0] - args[0]) as usize;
        bytes.copy_within(diff..end, 0);
        bytes[end - diff..end].fill(0);
        assert!(bytes.starts_with(b"/usr/lib/wine/wine64\0--version\0"));
        assert_eq!(&bytes[end..], b"A=1\0B=2\0");
    }

    #[test_case]
    fn linux_exec_string_crosses_stack_page_boundary() {
        let task = Task::new("linux-argv-pages".into(), 0, TaskType::User);
        task.allocate_stack_pages(USER_STACK_END - 2 * PAGE_SIZE, 2)
            .unwrap();
        let mut sp = USER_STACK_END - PAGE_SIZE + 2;
        let addresses = push_string_vector(&task, &mut sp, &["hello", ""]).unwrap();
        let mut bytes = [0; 7];
        copy_from_user(&task, sp, &mut bytes).unwrap();
        assert_eq!(&bytes, b"hello\0\0");
        assert_eq!(addresses, alloc::vec![sp as u64, (sp + 6) as u64]);
        let saved_sp = sp;
        assert!(push_string_vector(&task, &mut sp, &[]).unwrap().is_empty());
        assert_eq!(sp, saved_sp);
    }

    #[test_case]
    fn linux_exec_random_is_readable_in_new_image_before_auxv_terminator() {
        let task = Task::new("linux-auxv".into(), 0, TaskType::User);
        task.allocate_stack_pages(USER_STACK_END - PAGE_SIZE, 1)
            .unwrap();
        let mut sp = USER_STACK_END - 3;
        let mut auxv = alloc::vec![AuxVec::new(AT_NULL, 0)];
        add_random_auxv(&task, &mut sp, &mut auxv).unwrap();
        assert_eq!(sp & 15, 0);
        assert!(sp + 16 <= USER_STACK_END - 3);
        assert_eq!(auxv[0].a_type, AT_RANDOM);
        assert_eq!(auxv[0].a_val, sp as u64);
        assert_eq!(auxv[1].a_type, AT_NULL);
        let mut bytes = [0; 16];
        copy_from_user(&task, sp, &mut bytes).unwrap();
    }

    #[test_case]
    fn linux_exec_random_failure_does_not_publish_an_invalid_pointer() {
        let task = Task::new("linux-auxv-invalid".into(), 0, TaskType::User);
        let mut sp = USER_STACK_END;
        let mut auxv = alloc::vec![AuxVec::new(AT_NULL, 0)];
        assert!(add_random_auxv(&task, &mut sp, &mut auxv).is_err());
        assert_eq!(sp, USER_STACK_END);
        assert_eq!(auxv.len(), 1);
        assert_eq!(auxv[0].a_type, AT_NULL);
    }
}
