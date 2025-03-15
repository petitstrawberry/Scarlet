#[macro_use]
mod macros;

syscall_table! {
    Invalid = 0 => || 0,
}