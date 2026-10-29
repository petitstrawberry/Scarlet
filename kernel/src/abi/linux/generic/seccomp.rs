//! Linux seccomp cBPF conversion. Programs are copied once, verified before
//! publication, and evaluated on scalar call metadata (never user pointers).
use super::{LinuxAbi, errno};
use crate::{
    arch::Trapframe,
    library::std::usercopy::copy_from_user,
    shield::execution::{CallConstraint, Denial, Invocation, Verdict},
    task::{Task, mytask},
};
use alloc::{sync::Arc, vec, vec::Vec};

#[cfg(target_arch = "aarch64")]
const DOMAIN: &str = "linux-aarch64";
#[cfg(target_arch = "riscv64")]
const DOMAIN: &str = "linux-riscv64";
#[cfg(target_arch = "aarch64")]
pub const AUDIT_ARCH: u32 = 0xc00000b7;
#[cfg(target_arch = "riscv64")]
pub const AUDIT_ARCH: u32 = 0xc00000f3;
const MAX_INSNS: usize = 4096;
const DATA_SIZE: u32 = 64;
const ALLOW: u32 = 0x7fff0000;
const TRAP: u32 = 0x00030000;
const ERRNO: u32 = 0x00050000;
const KILL_PROCESS: u32 = 0x80000000;

#[derive(Clone, Copy, Debug)]
struct Instruction {
    code: u16,
    yes: u8,
    no: u8,
    value: u32,
}
struct Filter {
    instructions: Vec<Instruction>,
}
impl Filter {
    fn verify(instructions: Vec<Instruction>) -> Result<Self, usize> {
        let len = instructions.len();
        if len == 0 || len > MAX_INSNS {
            return Err(errno::EINVAL);
        }
        let mut initialized = vec![None; len];
        initialized[0] = Some(0u16);
        for (pc, insn) in instructions.iter().enumerate() {
            let mut mask = initialized[pc];
            let mut successors = [None, None];
            let next = pc + 1;
            match insn.code {
                0x20 => {
                    // LD W ABS: only aligned seccomp_data words.
                    if insn.value % 4 != 0 || insn.value >= DATA_SIZE {
                        return Err(errno::EINVAL);
                    }
                }
                0x00 | 0x01 | 0x80 | 0x81 | 0x87 | 0x07 => {} // imm, len, TAX/TXA
                0x60 | 0x61 => {
                    // Scratch must be initialized on every incoming path.
                    if insn.value >= 16 || mask.is_some_and(|m| m & (1 << insn.value) == 0) {
                        return Err(errno::EINVAL);
                    }
                }
                0x02 | 0x03 => {
                    if insn.value >= 16 {
                        return Err(errno::EINVAL);
                    }
                    mask = mask.map(|m| m | (1 << insn.value));
                }
                0x04 | 0x0c | 0x14 | 0x1c | 0x24 | 0x2c | 0x34 | 0x3c | 0x44 | 0x4c | 0x54
                | 0x5c | 0x64 | 0x6c | 0x74 | 0x7c | 0x84 | 0x94 | 0x9c | 0xa4 | 0xac => {
                    if matches!(insn.code, 0x34 | 0x94) && insn.value == 0 {
                        return Err(errno::EINVAL);
                    }
                    if matches!(insn.code, 0x64 | 0x74) && insn.value >= 32 {
                        return Err(errno::EINVAL);
                    }
                }
                0x05 => {
                    successors[0] = next.checked_add(insn.value as usize);
                }
                0x15 | 0x1d | 0x25 | 0x2d | 0x35 | 0x3d | 0x45 | 0x4d => {
                    successors = [
                        next.checked_add(insn.yes as usize),
                        next.checked_add(insn.no as usize),
                    ];
                }
                0x06 | 0x16 => {}
                _ => return Err(errno::EINVAL),
            }
            if insn.code & 7 == 6 {
                continue;
            }
            if insn.code & 7 != 5 {
                successors[0] = Some(next);
            }
            for target in successors.into_iter().flatten() {
                if target >= len {
                    return Err(errno::EINVAL);
                }
                if let Some(mask) = mask {
                    initialized[target] = Some(initialized[target].map_or(mask, |old| old & mask));
                }
            }
        }
        if instructions[len - 1].code & 7 != 6 {
            return Err(errno::EINVAL);
        }
        Ok(Self { instructions })
    }
    fn run(&self, data: &[u32; 16]) -> u32 {
        let (mut a, mut x) = (0u32, 0u32);
        let mut memory = [0u32; 16];
        let mut pc = 0;
        while let Some(i) = self.instructions.get(pc) {
            pc += 1;
            let rhs = if i.code & 8 == 0 { i.value } else { x };
            match i.code {
                0x20 => a = data[i.value as usize / 4],
                0x00 => a = i.value,
                0x01 => x = i.value,
                0x80 => a = DATA_SIZE,
                0x81 => x = DATA_SIZE,
                0x60 => a = memory[i.value as usize],
                0x61 => x = memory[i.value as usize],
                0x02 => memory[i.value as usize] = a,
                0x03 => memory[i.value as usize] = x,
                0x07 => x = a,
                0x87 => a = x,
                0x04 | 0x0c => a = a.wrapping_add(rhs),
                0x14 | 0x1c => a = a.wrapping_sub(rhs),
                0x24 | 0x2c => a = a.wrapping_mul(rhs),
                0x34 | 0x3c => {
                    if rhs == 0 {
                        return 0;
                    }
                    a /= rhs;
                }
                0x94 | 0x9c => {
                    if rhs == 0 {
                        return 0;
                    }
                    a %= rhs;
                }
                0x44 | 0x4c => a |= rhs,
                0x54 | 0x5c => a &= rhs,
                0xa4 | 0xac => a ^= rhs,
                0x64 | 0x6c => a = a.checked_shl(rhs).unwrap_or(0),
                0x74 | 0x7c => a = a.checked_shr(rhs).unwrap_or(0),
                0x84 => a = a.wrapping_neg(),
                0x05 => pc += i.value as usize,
                0x15 | 0x1d | 0x25 | 0x2d | 0x35 | 0x3d | 0x45 | 0x4d => {
                    let yes = match i.code & 0xf0 {
                        0x10 => a == rhs,
                        0x20 => a > rhs,
                        0x30 => a >= rhs,
                        _ => a & rhs != 0,
                    };
                    pc += if yes { i.yes } else { i.no } as usize;
                }
                0x06 => return i.value,
                0x16 => return a,
                _ => return KILL_PROCESS, // Unreachable after validation.
            }
        }
        KILL_PROCESS
    }
}
fn data(call: &Invocation) -> [u32; 16] {
    let mut words = [0; 16];
    words[0] = call.number as u32;
    words[1] = AUDIT_ARCH;
    // Linux exposes the saved return PC (after SVC), while the core records the callsite.
    let instruction = call.instruction.wrapping_add(4);
    words[2] = instruction as u32;
    words[3] = (instruction >> 32) as u32;
    for (i, arg) in call.arguments.iter().enumerate() {
        words[4 + i * 2] = *arg as u32;
        words[5 + i * 2] = (*arg >> 32) as u32;
    }
    words
}
impl CallConstraint for Filter {
    fn evaluate(&self, call: &Invocation) -> Verdict {
        // Native-tagged syscalls and exec into another ABI cannot bypass rules.
        if call.domain != DOMAIN {
            return Verdict::ABORT;
        }
        let result = self.run(&data(call));
        let detail = result & 0xffff;
        let (rank, denial) = match result & 0xffff0000 {
            ALLOW => return Verdict::ALLOW,
            0x7ffc0000 => {
                // LOG is allow with a kernel audit record.
                crate::println!(
                    "[seccomp] task={} syscall={} ip={:#x}",
                    mytask().map_or(0, |t| t.get_id()),
                    call.number,
                    call.instruction
                );
                return Verdict {
                    rank: 6,
                    denial: None,
                };
            }
            0 => (1, Denial::AbortThread),
            TRAP => (2, Denial::Exception { kind: 1, detail }),
            ERRNO => (
                3,
                Denial::Return(errno::to_result(detail.min(4095) as usize)),
            ),
            // No listener or ptrace backend exists; Linux also returns ENOSYS.
            0x7fc00000 => (4, Denial::Return(errno::to_result(errno::ENOSYS))),
            0x7ff00000 => (5, Denial::Return(errno::to_result(errno::ENOSYS))),
            _ => return Verdict::ABORT,
        };
        Verdict {
            rank,
            denial: Some(denial),
        }
    }
}

pub fn mode(task: &Task) -> usize {
    if task.execution_restrictions.read().is_constrained() {
        2
    } else {
        0
    }
}
pub fn set_mode(task: &Task, mode: usize, ptr: usize) -> usize {
    let result = match mode {
        2 => install(task, ptr),
        1 => Err(errno::EOPNOTSUPP),
        _ => Err(errno::EINVAL),
    };
    result.map_or_else(errno::to_result, |_| 0)
}
fn install(task: &Task, ptr: usize) -> Result<(), usize> {
    // RISC-V's legacy signal frames lack synchronous SIGSYS siginfo. Do not
    // advertise filtering until the adapter can implement every supported action.
    if !cfg!(target_arch = "aarch64") {
        return Err(errno::EOPNOTSUPP);
    }
    if !task.execution_restrictions.read().no_new_authority()
        && !task
            .credentials
            .read()
            .permits(crate::shield::identity::MANAGE_MOUNTS)
    {
        return Err(errno::EACCES);
    }
    let mut header = [0u8; 16];
    copy_from_user(task, ptr, &mut header).map_err(|_| errno::EFAULT)?;
    let len = u16::from_ne_bytes(header[..2].try_into().unwrap()) as usize;
    if len == 0 || len > MAX_INSNS {
        return Err(errno::EINVAL);
    }
    let address = u64::from_ne_bytes(header[8..].try_into().unwrap()) as usize;
    let mut bytes = vec![0; len * 8];
    copy_from_user(task, address, &mut bytes).map_err(|_| errno::EFAULT)?;
    let instructions = bytes
        .chunks_exact(8)
        .map(|b| Instruction {
            code: u16::from_ne_bytes(b[..2].try_into().unwrap()),
            yes: b[2],
            no: b[3],
            value: u32::from_ne_bytes(b[4..].try_into().unwrap()),
        })
        .collect();
    let filter = Filter::verify(instructions)?;
    task.execution_restrictions
        .write()
        .append(Arc::new(filter), len)
        .map_err(|_| errno::ENOMEM)
}
pub fn sys_seccomp(_: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let operation = frame.get_arg(0);
    let flags = frame.get_arg(1);
    let pointer = frame.get_arg(2);
    frame.increment_pc_next(&task);
    if flags != 0 {
        return errno::to_result(errno::EINVAL);
    } // TSYNC/listener flags unsupported.
    match operation {
        1 => set_mode(&task, 2, pointer),
        2 => {
            let mut bytes = [0; 4];
            if copy_from_user(&task, pointer, &mut bytes).is_err() {
                return errno::to_result(errno::EFAULT);
            }
            match u32::from_ne_bytes(bytes) {
                0 | KILL_PROCESS | TRAP | ERRNO | ALLOW | 0x7ffc0000 | 0x7fc00000 | 0x7ff00000
                    if cfg!(target_arch = "aarch64") =>
                {
                    0
                }
                _ => errno::to_result(errno::EOPNOTSUPP),
            }
        }
        _ => errno::to_result(errno::EINVAL),
    }
}
pub fn handle_denial(abi: &mut LinuxAbi, denial: Denial, frame: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let number = frame.get_syscall_number();
    let address = frame.get_current_pc() as usize + 4;
    frame.increment_pc_next(&task);
    match denial {
        Denial::Return(value) => {
            super::signal::deliver_pending_signals(abi);
            #[cfg(target_arch = "aarch64")]
            return crate::abi::linux::aarch64::signal::deliver_pending_handler(abi, frame, value);
            #[cfg(not(target_arch = "aarch64"))]
            return value;
        }
        Denial::Exception { kind: 1, detail } =>
        {
            #[cfg(target_arch = "aarch64")]
            if crate::abi::linux::aarch64::signal::deliver_seccomp_trap(
                abi, &task, frame, detail, number, address,
            )
            .is_ok()
            {
                return frame.get_return_value();
            }
        }
        Denial::AbortThread => {
            task.mark_signal_termination(31);
            task.request_deferred_exit(159);
            return 0;
        }
        _ => {}
    }
    task.mark_signal_termination(31);
    task.request_deferred_exit_group(159);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn i(code: u16, value: u32) -> Instruction {
        Instruction {
            code,
            value,
            yes: 0,
            no: 0,
        }
    }
    #[test_case]
    fn seccomp_verifier_rejects_oob_jumps_data_and_uninitialized_scratch() {
        for program in [
            vec![],
            vec![i(0x20, 64), i(6, ALLOW)],
            vec![i(0x20, 1), i(6, ALLOW)],
            vec![i(5, u32::MAX), i(6, ALLOW)],
            vec![i(0x60, 0), i(6, ALLOW)],
            vec![i(0x34, 0), i(6, ALLOW)],
            vec![i(0x64, 32), i(6, ALLOW)],
            vec![i(0, 1)],
            vec![i(0x28, 0), i(6, ALLOW)],
            vec![i(6, ALLOW); MAX_INSNS + 1],
        ] {
            assert!(Filter::verify(program).is_err());
        }
        let branch = Instruction {
            code: 0x15,
            value: 0,
            yes: 1,
            no: 0,
        };
        assert!(Filter::verify(vec![branch, i(2, 0), i(0x60, 0), i(6, ALLOW)]).is_err());
        assert!(Filter::verify(vec![i(2, 0), branch, i(2, 0), i(0x60, 0), i(6, ALLOW)]).is_ok());
    }
    #[test_case]
    fn seccomp_filters_arch_arguments_and_native_escape_without_dereferencing() {
        let program = vec![
            i(0x20, 4),
            Instruction {
                code: 0x15,
                value: AUDIT_ARCH,
                yes: 1,
                no: 0,
            },
            i(6, KILL_PROCESS),
            i(0x20, 20),
            Instruction {
                code: 0x15,
                value: 0x12345678,
                yes: 0,
                no: 1,
            },
            i(6, ERRNO | 13),
            i(6, ALLOW),
        ];
        let filter = Filter::verify(program).unwrap();
        let mut call = Invocation {
            domain: DOMAIN,
            number: 123,
            instruction: 0x123456789abcdef0,
            arguments: [0; 6],
        };
        assert!(filter.evaluate(&call).denial.is_none());
        call.arguments[0] = 0x12345678ffffffff;
        assert_eq!(
            filter.evaluate(&call).denial,
            Some(Denial::Return(errno::to_result(13)))
        );
        assert_eq!(data(&call)[2], 0x9abcdef4);
        call.domain = "scarlet-native";
        assert_eq!(filter.evaluate(&call).denial, Some(Denial::AbortProcess));
    }
    #[test_case]
    fn seccomp_arithmetic_is_32_bit_and_zero_divisors_cannot_panic() {
        let f = Filter::verify(vec![
            i(0, u32::MAX),
            i(4, 2),
            i(2, 0),
            i(1, 0),
            i(0x60, 0),
            i(0x3c, 0),
            i(0x16, 0),
        ])
        .unwrap();
        assert_eq!(f.run(&[0; 16]), 0);
        let f = Filter::verify(vec![
            i(0, u32::MAX),
            i(4, 2),
            i(2, 0),
            i(0x61, 0),
            i(0x0c, 0),
            i(0x16, 0),
        ])
        .unwrap();
        assert_eq!(f.run(&[0; 16]), 2);
        let f = Filter::verify(vec![i(0, 1), i(1, 32), i(0x6c, 0), i(0x16, 0)]).unwrap();
        assert_eq!(f.run(&[0; 16]), 0);
    }
}
