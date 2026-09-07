//! Launch helpers for native wrappers around programs in another ABI view.

use scarlet_os::{
    Handle,
    environment::{Environment, HandleMapping},
    handle::{HandleError, HandleResult},
};
use scarlet_sys::{Syscall, syscall1};

fn duplicate_stdio(raw: usize) -> HandleResult<Handle> {
    // SAFETY: duplication only borrows the standard descriptor for this call.
    let duplicate = unsafe { syscall1(Syscall::HandleDuplicate, raw) };
    if duplicate == usize::MAX {
        return Err(HandleError::SystemError(-1));
    }
    // SAFETY: the successful syscall returned a new, exclusively owned handle.
    unsafe { Handle::from_raw(duplicate as i32) }
}

/// Resolve the executable inside the selected ABI view, without consulting the
/// backing filesystem. Only standard streams cross the transition.
pub fn exec(
    abi: &str,
    path: &str,
    argv: &[&str],
    envp: &[&str],
    cwd: &str,
    input: Option<&Handle>,
) -> HandleResult<core::convert::Infallible> {
    let environment = Environment::current()?;
    let view = environment.root(abi)?;
    let executable = view.open(path, 0)?;
    let stdin = if input.is_none() {
        Some(duplicate_stdio(0)?)
    } else {
        None
    };
    let stdout = duplicate_stdio(1)?;
    let stderr = duplicate_stdio(2)?;
    let handles = [
        HandleMapping {
            source: input.or(stdin.as_ref()).unwrap(),
            target: 0,
        },
        HandleMapping {
            source: &stdout,
            target: 1,
        },
        HandleMapping {
            source: &stderr,
            target: 2,
        },
    ];
    environment.exec_with_abi(abi, &executable, argv, envp, cwd, &handles)
}
