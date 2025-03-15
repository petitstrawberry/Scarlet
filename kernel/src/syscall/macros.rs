macro_rules! syscall_table {
    ( $( $name:ident = $num:expr => $func:expr ),* $(,)? ) => {
        #[derive(Debug)]
        pub enum Syscall {
            $(
                $name = $num,
            )*
        }

        pub fn syscall_handler(trapframe: &mut Trapframe) -> Result<usize, &'static str> {
            let syscall_number = trapframe.get_arg(0);
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
                    Err("Invalid syscall number")
                }
            }
        }
    };
}