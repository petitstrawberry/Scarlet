# SCPM Library API Reference

## Core Types

### Config

Package manager configuration.

```rust
pub struct Config {
    pub installed_dir: String,
    pub cache_dir: String,
    pub bin_dir: String,
    pub lib_dir: String,
    pub registry_file: String,
}

impl Config {
    pub fn new() -> Self;
    pub fn default() -> Self;
    pub fn with_paths(...) -> Self;
}
```

### Error

Error types for package operations.

```rust
pub enum Error {
    PackageNotFound(String),
    PackageAlreadyInstalled(String),
    InvalidMetadata(String),
    IoError(String),
    InstallationFailed(String),
    RemovalFailed(String),
    DependencyError(String),
    InvalidPackageFormat(String),
    NetworkError(String),
}

pub type Result<T> = core::result::Result<T, Error>;
```

### PackageMetadata

Package metadata from package.toml.

```rust
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub bin_name: String,
    pub binaries: Vec<String>,
    pub libraries: Vec<String>,
    pub dependencies: Vec<Dependency>,
    pub architecture: String,
    pub license: Option<String>,
    pub installed_files: Vec<String>,  // Tracked after installation
}
```

### Dependency

Package dependency specification.

```rust
pub struct Dependency {
    pub name: String,
    pub version: Option<String>,
}
```

### Package

Package with metadata and optional installation path.

```rust
pub struct Package {
    pub metadata: PackageMetadata,
    pub install_path: Option<String>,
}

impl Package {
    pub fn new(metadata: PackageMetadata) -> Self;
    pub fn id(&self) -> String;  // Returns "name-version"
    pub fn archive_filename(&self) -> String;  // Returns "name-version.scarlet"
}
```

### PackageManager

Main package manager interface.

```rust
pub struct PackageManager {
    config: Config,
    installed_packages: Vec<PackageMetadata>,
    repository: RepositoryIndex,
}

impl PackageManager {
    pub fn new(config: Config) -> Self;
    pub fn with_default_config() -> Self;
    pub fn load_registry(&mut self) -> Result<()>;
    pub fn save_registry(&self) -> Result<()>;
    pub fn is_installed(&self, name: &str) -> bool;
    pub fn get_installed(&self, name: &str) -> Option<&PackageMetadata>;
    pub fn list_installed(&self) -> &[PackageMetadata];
    pub fn search(&self, query: &str) -> Vec<&RepoEntry>;
    pub fn install(&mut self, package: Package) -> Result<()>;
    pub fn install_from_bytes(&mut self, name: &str, data: &[u8]) -> Result<()>;
    pub fn remove(&mut self, name: &str) -> Result<()>;
    pub fn fetch_package(&self, name: &str) -> Result<Vec<u8>>;
    pub fn resolve_dependencies(&self, package: &Package) -> Vec<String>;
}
```

### Repository

Package repository interface.

```rust
pub struct Repository {
    pub url: String,
    pub packages: Vec<RepoEntry>,
}

pub struct RepoEntry {
    pub name: String,
    pub version: String,
    pub description: String,
}

pub struct RepositoryIndex {
    pub repositories: Vec<Repository>,
}

impl Repository {
    pub fn new(url: String) -> Self;
    pub fn search(&self, query: &str) -> Vec<&RepoEntry>;
    pub fn get_package(&self, name: &str) -> Option<&RepoEntry>;
}

impl RepositoryIndex {
    pub fn new() -> Self;
    pub fn add_repository(&mut self, repo: Repository);
    pub fn search_all(&self, query: &str) -> Vec<&RepoEntry>;
    pub fn get_package(&self, name: &str) -> Option<&RepoEntry>;
}
```

### PackageArchive

Archive operations for .scarlet packages (tar.gz format).

```rust
pub struct TarEntry {
    pub name: String,
    pub mode: u32,
    pub size: u64,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub data: Vec<u8>,
}

pub struct PackageArchive {
    pub metadata: PackageMetadata,
    pub entries: Vec<TarEntry>,
}

impl PackageArchive {
    pub fn from_bytes(data: &[u8]) -> Result<Self>;
    pub fn get_binary(&self, name: &str) -> Result<&[u8]>;
    pub fn get_library(&self, name: &str) -> Result<&[u8]>;
    pub fn extract_to(&self, dest_dir: &str) -> Result<()>;
    pub fn extract_root(&self, root_prefix: &str) -> Result<Vec<String>>;
}
```

## Usage Examples

### Basic Package Management

```rust
use scarlet_std as std;
use scpm::PackageManager;

fn main() {
    // Create package manager with default config
    let mut manager = PackageManager::with_default_config();
    
    // Check if package is installed
    if manager.is_installed("hello") {
        println!("Package 'hello' is already installed");
        return;
    }
    
    // Install package from .scarlet file
    let package_data = std::fs::read("hello-1.0.0.scarlet")?;
    match manager.install_from_bytes("hello", &package_data) {
        Ok(()) => println!("Package installed successfully"),
        Err(e) => println!("Installation failed: {}", e),
    }
    
    // List installed packages
    println!("\nInstalled packages:");
    for pkg in manager.list_installed() {
        println!("  {} - {}", pkg.name, pkg.version);
    }
    
    // Show installed files
    if let Some(pkg) = manager.get_installed("hello") {
        println!("\nInstalled files:");
        for file in &pkg.installed_files {
            println!("  {}", file);
        }
    }
}
```

### Custom Configuration

```rust
use scarlet_std as std;
use scpm::{Config, PackageManager};

fn main() {
    // Create custom configuration
    let config = Config {
        installed_dir: String::from("/opt/scpm/installed"),
        cache_dir: String::from("/opt/scpm/cache"),
        bin_dir: String::from("/usr/bin"),
        lib_dir: String::from("/usr/lib"),
        registry_file: String::from("/opt/scpm/registry.toml"),
    };
    
    let manager = PackageManager::new(config);
    
    // Use manager...
}
```

### Search in Repository

```rust
use scarlet_std as std;
use scpm::PackageManager;

fn main() {
    let manager = PackageManager::with_default_config();
    
    // Search for packages
    let results = manager.search("editor");
    println!("Search results for 'editor':");
    for entry in results {
        println!("  {} - {}", entry.name, entry.version);
        println!("    {}", entry.description);
    }
}
```

### Dependency Resolution

```rust
use scarlet_std as std;
use scpm::{PackageManager, Package, PackageMetadata, Dependency};

fn main() {
    let mut manager = PackageManager::with_default_config();
    
    let metadata = PackageMetadata {
        name: String::from("myapp"),
        version: String::from("1.0.0"),
        description: String::from("My Application"),
        author: None,
        homepage: None,
        binaries: vec![String::from("myapp")],
        libraries: vec![],
        dependencies: vec![
            Dependency {
                name: String::from("libcommon"),
                version: Some(String::from("1.0.0")),
            }
        ],
        architecture: String::from("riscv64"),
        license: None,
    };
    
    // Check dependencies before installing
    let package = Package::new(metadata);
    match manager.install(package) {
        Ok(()) => println!("Installed successfully"),
        Err(e) => println!("Failed: {}", e),
    }
}
```
