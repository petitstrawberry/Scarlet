# Cross-ABI Resource Management in Scarlet OS

## 概要

Scarlet OSにおけるマルチABI環境でのリソース管理設計。以下を包括的にカバー：
- ハンドル/ファイル記述子の透明な変換と継承
- VFS（仮想ファイルシステム）の共有とABI固有構造
- exec処理の統一化とバイナリ形式の多様性サポート
- ABI切り替え時の透明なリソース継承

## 設計原則

### リソース管理の責任分離
- **Scarlet Core**: HandleTable/KernelObject/VfsManagerの抽象化、統一exec処理
- **ABI Module**: 自分のABI固有の変換ロジックとバイナリ形式のみ
- Scarlet CoreはABI固有の変換処理について一切関知しない

### VFS設計原則
- **基本共有 + ABI固有拡張**: 基本的には全ABI間で同じファイルを共有、各ABIが独自部分を追加
- **exec時継承**: プロセス置換時に共有ファイルシステムを明示的に継承
- **既存VFS活用**: Scarletの既存VFS実装を最大限活用

### old_abi不要の原理
- `execve_abi`時点で既にHandleはScarletによって完全に抽象化済み
- 新しいABIは既存の`HandleTable`状況を見て自分で変換テーブルを構築
- 以前のABIの情報は不要

## アーキテクチャ

### コアカーネル (不変)
```rust
pub struct HandleTable {
    handles: [Option<KernelObject>; 1024],
    free_handles: Vec<Handle>,
}

pub enum KernelObject {
    File(Arc<dyn FileObject>),
    Pipe(Arc<dyn PipeObject>),
    // 将来の拡張...
}
```

### ABI Module トレイト
```rust
pub trait AbiModule: 'static + Send + Sync {
    fn name() -> &'static str where Self: Sized;
    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str>;
    fn init(&self) {}
    fn init_fs(&self, _vfs: &mut VfsManager) {}
    
    // ABI切り替え時のハンドル変換処理
    fn initialize_from_existing_handles(&self, task: &Task) -> Result<(), &'static str> {
        Ok(()) // デフォルト: 変換不要
    }
    
    // バイナリ実行処理（各ABIが独自のバイナリ形式をサポート）
    fn execute_binary(
        &self,
        path: &str,
        argv: &[&str], 
        envp: &[&str],
        task: &Task,
        trapframe: &mut Trapframe
    ) -> Result<(), &'static str>;
    
    // VFS関連メソッド
    
    /// ベースVFSからABI固有VFSを作成
    fn create_abi_vfs_from_base(&self, base_vfs: BaseVfs) -> Result<Arc<VfsManager>, &'static str> {
        let mut vfs = VfsManager::new();
        base_vfs.apply_to_vfs(&mut vfs)?;
        self.setup_base_directories(&mut vfs)?;
        self.mount_abi_specific_filesystems(&mut vfs)?;
        Ok(Arc::new(vfs))
    }
    
    /// 初期VFS作成（親プロセスなし）
    fn create_initial_abi_vfs(&self) -> Result<Arc<VfsManager>, &'static str> {
        let mut vfs = VfsManager::new();
        let rootfs_id = vfs.create_and_register_fs("tmpfs", &TmpFSParams::default())?;
        vfs.mount(rootfs_id, "/")?;
        self.setup_base_directories(&mut vfs)?;
        self.mount_abi_specific_filesystems(&mut vfs)?;
        Ok(Arc::new(vfs))
    }
    
    /// 基本ディレクトリ構造の作成
    fn setup_base_directories(&self, vfs: &mut VfsManager) -> Result<(), &'static str> {
        // デフォルト実装: Unix風ディレクトリ構造
        vfs.mkdir("/home")?;
        vfs.mkdir("/tmp")?;
        vfs.mkdir("/etc")?;
        vfs.mkdir("/var")?;
        vfs.mkdir("/var/log")?;
        vfs.mkdir("/usr")?;
        vfs.mkdir("/usr/share")?;
        Ok(())
    }
    
    /// ABI固有ファイルシステムのマウント
    fn mount_abi_specific_filesystems(&self, vfs: &mut VfsManager) -> Result<(), &'static str> {
        // デフォルト実装: 何もしない
        Ok(())
    }
    
    /// ABI固有のデフォルトworking directory
    fn get_default_cwd(&self) -> &str {
        "/" // デフォルトはルート
    }
}
```

### VFS継承用の構造体
```rust
/// exec時に継承される共有VFS情報
pub struct BaseVfs {
    shared_mounts: Vec<MountEntry>,
}

impl BaseVfs {
    pub fn new(shared_mounts: Vec<MountEntry>) -> Self {
        Self { shared_mounts }
    }
    
    /// ベースVFSの内容を新しいVfsManagerに適用
    pub fn apply_to_vfs(&self, vfs: &mut VfsManager) -> Result<(), &'static str> {
        // ベースとなるルートファイルシステムを作成
        let rootfs_id = vfs.create_and_register_fs("tmpfs", &TmpFSParams::default())?;
        vfs.mount(rootfs_id, "/")?;
        
        // 共有マウントを再適用
        for mount_entry in &self.shared_mounts {
            // 共有ファイルシステムを新しいVFSにマウント
            vfs.mount_shared(mount_entry.filesystem.clone(), &mount_entry.mount_point)?;
        }
        
        Ok(())
    }
}
```

## 実装例

### xv6 ABI (変換不要、Unix風VFS)
```rust
impl AbiModule for Xv6Riscv64Abi {
    fn name() -> &'static str { "xv6-riscv64" }
    
    fn initialize_from_existing_handles(&self, task: &Task) -> Result<(), &'static str> {
        // fd = handle の直接マッピングなので変換不要
        Ok(())
    }
    
    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        // 直接Handleを使用（変換なし）
        xv6_syscall_handler(trapframe)
    }
    
    fn execute_binary(&self, path: &str, argv: &[&str], envp: &[&str], 
                      task: &Task, trapframe: &mut Trapframe) -> Result<(), &'static str> {
        // ELFバイナリをロードして実行
        let file = task.vfs.as_ref().unwrap().open(path, 0)?;
        let file_obj = file.as_file().unwrap();
        
        // ELFロードとプロセス置換
        let entry_point = load_elf_into_task(file_obj, task)?;
        setup_new_process_context(task, entry_point, argv, envp, trapframe);
        
        Ok(()) // 成功時は制御が戻らない
    }
    
    // VFS関連はデフォルト実装を使用（Unix風ディレクトリ構造）
    fn get_default_cwd(&self) -> &str {
        "/home/user"
    }
}
```

### Windows ABI (変換あり、Windows風VFS)
```rust
impl AbiModule for WindowsNt32Abi {
    fn name() -> &'static str { "windows-nt32" }
    
    fn initialize_from_existing_handles(&self, task: &Task) -> Result<(), &'static str> {
        let mut translation_table = HashMap::new();
        
        // 既存の全Handleに対してWindows HANDLEを割り当て
        for handle in task.handle_table.active_handles() {
            if let Some(kernel_obj) = task.handle_table.get(handle) {
                let windows_handle = match kernel_obj {
                    KernelObject::File(_) => self.allocate_file_handle(),
                    KernelObject::Pipe(_) => self.allocate_pipe_handle(),
                };
                translation_table.insert(windows_handle, handle);
            }
        }
        
        self.update_handle_translation_table(translation_table)?;
        Ok(())
    }
    
    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        // Windows HANDLE → Scarlet Handle変換を行ってからコア処理
        windows_syscall_handler(trapframe, &self.translation_table)
    }
    
    fn execute_binary(&self, path: &str, argv: &[&str], envp: &[&str], 
                      task: &Task, trapframe: &mut Trapframe) -> Result<(), &'static str> {
        // PEバイナリをロードして実行
        let file = task.vfs.as_ref().unwrap().open(path, 0)?;
        let file_obj = file.as_file().unwrap();
        
        // PEロードとプロセス置換
        let entry_point = load_pe_into_task(file_obj, task)?;
        setup_windows_process_context(task, entry_point, argv, envp, trapframe);
        
        Ok(()) // 成功時は制御が戻らない
    }
    
    // Windows固有VFS構造
    fn setup_base_directories(&self, vfs: &mut VfsManager) -> Result<(), &'static str> {
        // Windows風ディレクトリ構造
        vfs.mkdir("/C:")?;
        vfs.mkdir("/C:/Windows")?;
        vfs.mkdir("/C:/Users")?;
        vfs.mkdir("/C:/Program Files")?;
        vfs.mkdir("/C:/Temp")?;
        Ok(())
    }
    
    fn mount_abi_specific_filesystems(&self, vfs: &mut VfsManager) -> Result<(), &'static str> {
        // Windows Registry ファイルシステム
        let registry_id = vfs.create_and_register_fs_with_params(
            "windows_registry",
            &WindowsRegistryParams::default()
        )?;
        vfs.mount(registry_id, "/Registry")?;
        
        Ok(())
    }
    
    fn get_default_cwd(&self) -> &str {
        "/C:/Users/user"
    }
}
```

### Linux ABI (Linuxシステム拡張)
```rust
impl AbiModule for LinuxAbi {
    fn name() -> &'static str { "linux" }
    
    fn mount_abi_specific_filesystems(&self, vfs: &mut VfsManager) -> Result<(), &'static str> {
        // Linux標準の /proc ファイルシステム
        let linux_procfs_id = vfs.create_and_register_fs_with_params(
            "linux_procfs",
            &LinuxProcFSParams::default()
        )?;
        vfs.mount(linux_procfs_id, "/proc")?;
        
        // Linux標準の /sys ファイルシステム
        let linux_sysfs_id = vfs.create_and_register_fs_with_params(
            "linux_sysfs", 
            &LinuxSysFSParams::default()
        )?;
        vfs.mount(linux_sysfs_id, "/sys")?;
        
        // Linux標準の /dev ファイルシステム
        let linux_devfs_id = vfs.create_and_register_fs_with_params(
            "linux_devfs",
            &LinuxDevFSParams::default()
        )?;
        vfs.mount(linux_devfs_id, "/dev")?;
        
        Ok(())
    }
    
    fn get_default_cwd(&self) -> &str {
        "/home/user"
    }
}
```

## ABI切り替え処理

### TransparentExecutor: 統一exec API

全てのABI moduleがexecを実行する際は、コアカーネルの`TransparentExecutor`を使用する：

```rust
pub struct TransparentExecutor;

impl TransparentExecutor {
    /// バイナリを解析し、適切なABIを自動検出してexecの前処理を実行
    pub fn execute_binary(
        path: &str, 
        argv: &[&str], 
        envp: &[&str],
        current_task: &Task,
        trapframe: &mut Trapframe
    ) -> Result<(), ExecError> {
        // 1. バイナリ解析とABI自動検出
        let binary_info = BinaryAnalyzer::analyze(&path)?;
        
        // 2. VFS継承準備
        let base_vfs = Self::extract_base_vfs_for_exec(current_task)?;
        
        // 3. 必要に応じてABI切り替え
        let target_abi_name = binary_info.detected_abi;
        let current_abi_name = current_task.abi.as_ref().map(|a| a.name());
        
        if current_abi_name != Some(target_abi_name) {
            let target_abi = AbiRegistry::instantiate(target_abi_name)?;
            target_abi.initialize_from_existing_handles(current_task)?;
            
            // VFS設定
            let new_vfs = if let Some(base) = base_vfs {
                target_abi.create_abi_vfs_from_base(base)?
            } else {
                target_abi.create_initial_abi_vfs()?
            };
            current_task.vfs = Some(new_vfs);
            current_task.cwd = Some(target_abi.get_default_cwd().to_string());
            current_task.abi = Some(target_abi);
        }
        
        // 4. 実際のexec実行はABI moduleに委任
        let abi = current_task.abi.as_ref().unwrap();
        abi.execute_binary(path, argv, envp, current_task, trapframe)
    }
    
    /// ABI指定でexecの前処理を実行
    pub fn execute_with_abi(
        path: &str, 
        argv: &[&str], 
        envp: &[&str],
        target_abi: &str,
        current_task: &Task,
        trapframe: &mut Trapframe
    ) -> Result<(), ExecError> {
        // VFS継承準備
        let base_vfs = Self::extract_base_vfs_for_exec(current_task)?;
        
        // ABI切り替え
        let abi = AbiRegistry::instantiate(target_abi)?;
        abi.initialize_from_existing_handles(current_task)?;
        
        // VFS設定
        let new_vfs = if let Some(base) = base_vfs {
            abi.create_abi_vfs_from_base(base)?
        } else {
            abi.create_initial_abi_vfs()?
        };
        current_task.vfs = Some(new_vfs);
        current_task.cwd = Some(abi.get_default_cwd().to_string());
        current_task.abi = Some(abi);
        
        // 実際のexec実行はABI moduleに委任
        abi.execute_binary(path, argv, envp, current_task, trapframe)
    }
    
    /// exec時の共有VFS情報抽出
    fn extract_base_vfs_for_exec(task: &Task) -> Result<Option<BaseVfs>, ExecError> {
        if let Some(current_vfs) = &task.vfs {
            // 共有対象のマウントポイントを抽出
            let shared_paths = [
                "/home",     // ユーザーファイル
                "/tmp",      // 一時ファイル  
                "/etc",      // 設定ファイル（読み取り専用として共有）
                "/var/log",  // ログファイル（共有）
                "/usr/share", // 共有データ
            ];
            
            match current_vfs.extract_shared_mounts(&shared_paths) {
                Ok(shared_mounts) => Ok(Some(BaseVfs::new(shared_mounts))),
                Err(_) => Ok(None), // 抽出失敗時は継承しない
            }
        } else {
            Ok(None) // 初期プロセスの場合
        }
    }
}
```

### ABI Module実装パターン

```rust
impl AbiModule for Xv6Riscv64Abi {
    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        match parse_syscall_number(trapframe) {
            XvSixSyscall::Exec => {
                let task = mytask().unwrap();
                let path = extract_path_from_trapframe(trapframe, task)?;
                let argv = extract_argv_from_trapframe(trapframe, task)?;
                let envp = extract_envp_from_trapframe(trapframe, task)?;
                
                // TransparentExecutorに委任
                match TransparentExecutor::execute_binary(&path, &argv, &envp, task, trapframe) {
                    Ok(()) => trapframe.get_return_value(),
                    Err(_) => usize::MAX, // exec失敗
                }
            },
            // 他のsyscallの処理...
        }
    }
}

impl AbiModule for WindowsNt32Abi {
    fn handle_syscall(&self, trapframe: &mut Trapframe) -> Result<usize, &'static str> {
        match parse_syscall_number(trapframe) {
            WindowsSyscall::CreateProcess => {
                let task = mytask().unwrap();
                let path = extract_windows_path_from_trapframe(trapframe, task)?;
                
                // TransparentExecutorに委任
                match TransparentExecutor::execute_binary(&path, &[], &[], task, trapframe) {
                    Ok(()) => trapframe.get_return_value(),
                    Err(_) => windows_error_code(),
                }
            },
            // 他のsyscallの処理...
        }
    }
}
```

## 実際の動作例

**xv6プロセス**:
```rust
// xv6 ABI環境でファイルオープン
let fd = open("/tmp/test.txt", O_RDWR); // fd=3, Handle=3

if (fork() == 0) {
    // Windows ABIに切り替え（TransparentExecutor経由）
    exec("/windows/app.exe", args);
    
    // 失敗時のみ到達
    printf("exec failed\n");
    exit(1);
}
```

**Windows process内**:
```c
/* 
WindowsNt32Abi::initialize_from_existing_handles()により
Handle=3 → Windows HANDLE 0x000012C4 への変換テーブルが自動作成済み
*/
int main() {
    DWORD bytes_read;
    char buffer[1024];
    
    // Handle=3が自動的にWindows HANDLE 0x000012C4に変換済み
    HANDLE hFile = ...; // 適切なHANDLE値が設定済み
    ReadFile(hFile, buffer, 1024, &bytes_read, NULL);
    return 0;
}
```

**実行フロー**:
1. xv6 ABI: `sys_exec` → `TransparentExecutor::execute_with_abi`
2. TransparentExecutor: バイナリ解析 → ABI切り替え → 新プロセス実行
3. 新プロセス: Windows ABI環境で開始、HandleTable継承済み

### Scarlet Native Syscall実装

Scarlet Native ABIのsyscallも同じくTransparentExecutorに委任：

```rust
// kernel/src/task/syscall.rs
pub fn sys_execve(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path = extract_path_from_trapframe(trapframe, task).unwrap_or_default();
    let argv = extract_argv_from_trapframe(trapframe, task).unwrap_or_default();
    let envp = extract_envp_from_trapframe(trapframe, task).unwrap_or_default();
    
    // TransparentExecutorに委任
    match TransparentExecutor::execute_binary(&path, &argv, &envp, task, trapframe) {
        Ok(()) => trapframe.get_return_value(),
        Err(_) => usize::MAX, // exec失敗
    }
}

pub fn sys_execve_abi(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path = extract_path_from_trapframe(trapframe, task).unwrap_or_default();
    let argv = extract_argv_from_trapframe(trapframe, task).unwrap_or_default();
    let envp = extract_envp_from_trapframe(trapframe, task).unwrap_or_default();
    let abi_name = extract_abi_from_trapframe(trapframe, task).unwrap_or_default();
    
    // TransparentExecutorに委任
    match TransparentExecutor::execute_with_abi(&path, &argv, &envp, &abi_name, task, trapframe) {
        Ok(()) => trapframe.get_return_value(),
        Err(_) => usize::MAX, // exec失敗
    }
}
```

## 設計の利点

### リソース管理の統合性
- **ハンドル管理**: 各ABIが独自変換テーブルで効率的変換
- **VFS管理**: exec時継承による自然なファイルシステム共有
- **統一exec処理**: 全ABIが同一の前処理ロジックを経由
- **バイナリ形式の自由度**: 各ABIが独自形式（ELF、PE、独自形式など）を完全サポート

### 責任の明確分離
- コアカーネルは抽象化のみ、ABI固有ロジックを一切知らない
- 各ABI moduleは自分の変換処理のみに専念
- 相互依存関係の完全排除
- **ABI moduleはScarlet Native syscallを直接呼ばない**
- **全execリクエストは統一TransparentExecutor経由**

### VFS継承モデルの利点
- **自然なプロセスモデル**: exec時のリソース継承の一環としてVFSも継承
- **柔軟な共有制御**: どの部分を継承するかをexec時に動的に決定
- **メモリ効率**: 必要な部分のみ継承、不要な部分は破棄
- **分離の明確性**: ABI固有部分は確実に分離、共有部分は明示的に継承

### exec処理の統一化
- バイナリ解析・ABI検出・ロード処理の一元化
- ABI横断的なバイナリ実行の透明サポート
- ELFロード等の複雑な処理の重複排除
- **重要**: ABI moduleがScarlet Native syscallを直接呼ぶことを禁止

### パフォーマンス
- 変換不要なABI (xv6) はオーバーヘッド皆無
- 変換必要なABI (Windows) はO(1)のHashMap lookup
- ABI切り替え時のみ変換コスト発生
- VFS継承は必要な部分のみでメモリ効率的

### 拡張性
- 新ABI追加時にコアカーネル変更不要
- ABI間の相互認識不要
- 型安全性によるコンパイル時エラー検出
- **各ABIが独自バイナリ形式をサポート可能**（ELF、PE、Mach-O、独自形式など）
- **各ABIが独自VFS構造をサポート可能**（Unix、Windows、独自形式など）

この設計により、Scarlet OSは真に拡張可能で保守しやすいマルチABI・マルチVFSアーキテクチャを実現する。

## アーキテクチャの要点

### 解決した問題
1. **循環依存の排除**: ABI moduleがScarlet Native syscallを直接呼ぶことを禁止
2. **exec処理の統一化**: 全ABIがTransparentExecutorを経由して一貫した動作を保証
3. **境界の明確化**: コアカーネルとABI moduleの責任範囲を明確に分離
4. **VFS継承の自然な実装**: exec時のリソース継承モデルでファイルシステムも管理

### このアーキテクチャのメリット
- **拡張性**: 新ABI追加時にコアカーネル・既存ABI双方に変更不要
- **保守性**: 各ABI moduleは独立しており、相互影響なく開発可能
- **一貫性**: 全execリクエストが同一ロジック（TransparentExecutor）で前処理される
- **型安全性**: コンパイル時に不正なABI間呼び出しを検出
- **バイナリ形式の自由度**: 各ABIが独自のバイナリ形式（ELF、PE、独自形式など）を完全サポート
- **VFS柔軟性**: 各ABIが独自のファイルシステム構造を自由に構築
- **リソース効率**: exec時継承により必要なリソースのみを共有
