macro_rules! syscall_table {
    ( $( $name:ident = $num:expr => $func:expr ),* $(,)? ) => {
        #[derive(Debug)]
        pub enum Syscall {
            $(
                $name = $num,
            )*
        }

        pub fn syscall_handler(syscall_number: usize) -> Result<usize, isize> {
            match syscall_number {
                $(
                    $num => {
                        $func()
                    }
                )*
                _ => {
                    Err(-2) /* Unknown syscall error */
                }
            }
        }
    };
}