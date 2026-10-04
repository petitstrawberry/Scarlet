//! Linux process-entry data which needs storage in the new user stack.

use crate::executor::executor::MAX_EXEC_STRINGS;
use crate::library::std::string::{StringConversionError, parse_c_string_from_userspace};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::task::Task;
use crate::task::elf_loader::{AT_NULL, AT_RANDOM, AuxVec};
use alloc::{string::String, vec::Vec};

/// Read an exec vector with the same count limit as the transactional loader.
/// argv and envp share a byte budget, including each terminating NUL. Command
/// arguments are not pathnames and may legitimately exceed the path limit.
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
        if strings.len() == MAX_EXEC_STRINGS {
            return Err(StringConversionError::TooManyStrings);
        }
        if *remaining == 0 {
            return Err(StringConversionError::ExceedsMaxLength);
        }
        let string = parse_c_string_from_userspace(task, pointer, *remaining)?;
        *remaining -= string.len() + 1;
        strings.push(string);
    }
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
