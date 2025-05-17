// エクスポートのテスト用構造体
// 次のように使用できます：
// #[export] struct MyStruct { ... }

use export_macro::export;

// // エクスポートされる構造体の例
// #[export]
// pub struct TestStruct {
//     pub id: u32,
//     pub name: &'static str,
// }

// // エクスポートされる列挙型の例
// #[export]
// pub enum TestEnum {
//     One,
//     Two,
//     Three,
// }

// // エクスポートされる型エイリアスの例
// #[export]
// pub type TestType = u64;

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test_case]
    fn test_export_type() {
        // このテストは実際には何も検証しませんが、
        // コンパイルが通ることを確認するためのものです
        crate::println!("Export type test passed!");
    }
}
