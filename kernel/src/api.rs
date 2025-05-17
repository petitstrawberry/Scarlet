// テスト用のAPI関数（xtaskのテスト用）
use export_macro::export;

/// テスト用のprint関数（xtaskのテスト用）
#[export]
pub fn kernel_print(msg: &str) {
    crate::println!("{}", msg);
}

#[export]
pub fn kernel_shutdown() -> ! {
    crate::arch::shutdown()
}

#[export]
pub fn kernel_panic(msg: &str) -> ! {
    crate::panic!(msg)
}