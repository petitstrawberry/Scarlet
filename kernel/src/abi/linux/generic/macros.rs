/// Define a syscall dispatch table returning `Option<usize>`.
///
/// Returns `Some(result)` on match, `None` otherwise.
/// Enables two-phase dispatch: common table first, then arch-specific fallback.
macro_rules! syscall_table {
    ( $handler_name:ident, $( $name:ident = $num:expr => $func:expr ),* $(,)? ) => {
        #[allow(dead_code)]
        #[derive(Debug)]
        pub enum Syscall {
            $(
                $name = $num,
            )*
        }

        pub fn $handler_name(abi: &mut crate::abi::linux::generic::LinuxAbi, trapframe: &mut crate::arch::Trapframe, syscall_number: usize) -> Option<usize> {
            match syscall_number {
                $(
                    $num => Some($func(abi, trapframe)),
                )*
                _ => None,
            }
        }
    };
}
