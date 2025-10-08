/// Define syscall table and syscall handler for WASI Preview 1
///
/// # Example
/// ```
/// syscall_table! {
///    Invalid = 0 => |_abi: &mut WasiPreview1Abi, _trapframe: &mut Trapframe| {
///       0
///   },
///   SomeSyscall = 1 => sys_somecall,
/// }
/// ```
macro_rules! syscall_table {
    ( $( $name:ident = $num:expr => $func:expr ),* $(,)? ) => {
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
        pub fn syscall_handler(abi: &mut crate::abi::wasi::preview1::WasiPreview1Abi, trapframe: &mut crate::arch::Trapframe) -> Result<usize, &'static str> {
            let syscall_number = trapframe.get_arg(7);
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
                    crate::println!("WASI syscall number: {}", syscall_number);
                    Err("Invalid syscall number")
                }
            }
        }
    };
}
