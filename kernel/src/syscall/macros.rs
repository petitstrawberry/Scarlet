macro_rules! syscall_table {
    ( $( $name:ident = $num:expr => $func:expr ),* $(,)? ) => {
        #[derive(Debug)]
        pub enum Syscall {
            $(
                $name = $num,
            )*
        }

        pub fn syscall_handler(syscall_number: usize) -> Result<isize, &'static str> {
            if syscall_number == 0 {
                return Err("Invalid syscall number");
            }
            match syscall_number {
                $(
                    $num => {
                        Ok($func())
                    }
                )*
                _ => {
                    Err("Invalid syscall number")
                }
            }
        }
    };
}