use crate::syscall::{syscall0, syscall1, Syscall};

/// プロセスをクローンします。
/// 
/// # 戻り値
/// - 親プロセスでは: 子プロセスのID
/// - 子プロセスでは: 0
/// - エラー時: -1（usize::MAX）
pub fn clone() -> usize {
    syscall0(Syscall::Clone)
}

/// 現在のプロセスを終了します。
/// 
/// # 引数
/// * `code` - 終了コード
pub fn exit(code: i32) -> ! {
    syscall1(Syscall::Exit, code as usize);
    unreachable!("exit syscall should not return");
}

/// プロセスの現在のIDを返します。
/// 注：この実装はダミーです。実際のpid取得システムコールが
/// 実装されるまでは常に1を返します。
pub fn getpid() -> usize {
    // 実際のgetpidシステムコールが実装されるまでのダミー実装
    1
}
