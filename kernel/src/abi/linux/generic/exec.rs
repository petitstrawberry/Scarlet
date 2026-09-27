//! Linux process-entry data which needs storage in the new user stack.

use crate::library::std::usercopy::copy_to_user;
use crate::task::Task;
use crate::task::elf_loader::{AT_NULL, AT_RANDOM, AuxVec};
use alloc::vec::Vec;

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
