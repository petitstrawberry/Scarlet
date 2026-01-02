/// Define syscall table and syscall handler for Linux ABI (architecture-independent)
///
/// # Example
/// ```
/// syscall_table! {
///    LinuxRiscv64Abi, linux::riscv64,
///    Invalid = 0 => |_:&mut LinuxRiscv64Abi, _: &mut Trapframe| {
///       0
///   },
///   SomeSyscall = 1 => sys_somecall,
/// }
/// ```
macro_rules! syscall_table {
    ( $abi_type:ty, $abi_path:path, $( $name:ident = $num:expr => $func:expr ),* $(,)? ) => {
        #[derive(Debug)]
        pub enum Syscall {
            $(
                $name = $num,
            )*
        }

        /// Syscall handler
        ///
        /// # Arguments
        /// * `abi` - The ABI module instance
        /// * `trapframe` - The trapframe
        ///
        /// # Returns
        /// The result of the syscall handler
        ///
        /// # Errors
        /// Returns an error if the syscall number is invalid
        pub fn syscall_handler(abi: &mut $abi_type, trapframe: &mut crate::arch::Trapframe) -> Result<usize, &'static str> {
            let syscall_number = trapframe.get_syscall_number();
            // crate::println!("Syscall number: {}", syscall_number);
            if syscall_number == 0 {
                return Err("Invalid syscall number");
            }
            match syscall_number {
                $(
                    $num => {
                        Ok($func(abi, trapframe))
                    }
                )*
                _ => {
                    crate::println!("Syscall number: {}", syscall_number);
                    Err("Invalid syscall number")
                }
            }
        }
    };
}
