#[macro_use]
mod macros;

syscall_table! {
    Invalid = 0 => || {
        Err(-1) /* Invalid syscall number */
    },
}