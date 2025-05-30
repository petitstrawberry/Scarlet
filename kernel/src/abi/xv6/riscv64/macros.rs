/// Define syscall table and syscall handler for xv6-riscv64
///
/// # Example
/// ```
/// syscall_table! {
///    Invalid = 0 => |_: &mut Trapframe| {
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
        /// * `trapframe` - The trapframe
        /// 
        /// # Returns
        /// The result of the syscall handler
        /// 
        /// # Errors
        /// Returns an error if the syscall number is invalid
        pub fn syscall_handler(trapframe: &mut crate::arch::Trapframe) -> Result<usize, &'static str> {
            let syscall_number = trapframe.get_arg(7);
            // crate::println!("Syscall number: {}", syscall_number);
            if syscall_number == 0 {
                return Err("Invalid syscall number");
            }
            match syscall_number {
                $(
                    $num => {
                        Ok($func(trapframe))
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