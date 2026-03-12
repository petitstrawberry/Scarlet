use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};
use serde::Deserialize;

#[derive(Parser, Debug)]
#[command(name = "cargo-scarlet")]
#[command(bin_name = "cargo-scarlet")]
#[command(about = "Prototype Scarlet build system generator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Generate {
        #[arg(long)]
        project: PathBuf,
    },
    Check {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        release: bool,
    },
    Build {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        release: bool,
    },
    Clippy {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        release: bool,
        #[arg(last = true)]
        extra_args: Vec<String>,
    },
    Run {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        release: bool,
        #[arg(last = true)]
        extra_args: Vec<String>,
    },
    Init {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        board: String,
    },
}

#[derive(Debug, Deserialize)]
struct ScarletConfig {
    config_version: u32,
    project: ProjectConfig,
    board: BoardConfig,
    kernel: KernelConfig,
    #[serde(rename = "modules", default)]
    modules: BTreeMap<String, ModuleConfig>,
}

#[derive(Debug, Deserialize)]
struct ProjectConfig {
    name: String,
}

#[derive(Debug, Deserialize)]
struct BoardConfig {
    name: String,
    target: String,
    target_json: String,
}

#[derive(Debug, Deserialize)]
struct KernelConfig {
    package: String,
    source: KernelSource,
    #[serde(default)]
    features: BTreeMap<String, bool>,
}

#[derive(Debug, Deserialize)]
struct KernelSource {
    version: Option<String>,
    path: Option<String>,
    git: Option<String>,
    rev: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    registry: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModuleConfig {
    enabled: bool,
    package: Option<String>,
    version: Option<String>,
    path: Option<String>,
    git: Option<String>,
    rev: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    registry: Option<String>,
    features: Option<Vec<String>>,
    #[serde(rename = "default-features")]
    default_features: Option<bool>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cargo-scarlet: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Generate { project } => {
            generate(&project)?;
            Ok(())
        }
        Commands::Check {
            project,
            target,
            release,
        } => {
            let config = generate(&project)?;
            cargo_command(&project, &config, "check", target, release, &[])
        }
        Commands::Build {
            project,
            target,
            release,
        } => {
            let config = generate(&project)?;
            cargo_command(&project, &config, "build", target, release, &[])
        }
        Commands::Clippy {
            project,
            target,
            release,
            extra_args,
        } => {
            let config = generate(&project)?;
            cargo_command(&project, &config, "clippy", target, release, &extra_args)
        }
        Commands::Run {
            project,
            target,
            release,
            extra_args,
        } => {
            let config = generate(&project)?;
            cargo_command(&project, &config, "run", target, release, &extra_args)
        }
        Commands::Init { project, board } => init_project(&project, &board),
    }
}

fn init_project(project: &Path, board: &str) -> Result<(), String> {
    let project = project.to_path_buf();
    ensure_init_target_is_safe(&project)?;

    fs::create_dir_all(project.join("src"))
        .map_err(|error| format!("failed to create {}: {error}", project.display()))?;
    fs::create_dir_all(project.join(".scarlet"))
        .map_err(|error| format!("failed to create {}: {error}", project.display()))?;

    let cargo_toml = render_init_cargo_toml(board);
    let main_rs = render_init_main_rs(board)?;
    let scarlet_config = render_init_config(board)?;
    let gitignore = ".scarlet\ntarget\n";

    write_if_changed(&project.join("Cargo.toml"), &cargo_toml)?;
    write_if_changed(&project.join("src/main.rs"), &main_rs)?;
    write_if_changed(&project.join("scarlet-config.toml"), &scarlet_config)?;
    write_if_changed(&project.join(".gitignore"), gitignore)?;

    Ok(())
}

fn generate(project: &Path) -> Result<ScarletConfig, String> {
    let project = normalize_project_path(project)?;
    let config_path = project.join("scarlet-config.toml");
    let config_text = fs::read_to_string(&config_path)
        .map_err(|error| format!("failed to read {}: {error}", config_path.display()))?;
    let config: ScarletConfig = toml::from_str(&config_text)
        .map_err(|error| format!("failed to parse {}: {error}", config_path.display()))?;

    validate_config(&config)?;

    let generated_root = project.join(".scarlet/scarlet-modules");
    let generated_src = generated_root.join("src");
    fs::create_dir_all(&generated_src)
        .map_err(|error| format!("failed to create {}: {error}", generated_src.display()))?;

    let cargo_toml = render_generated_manifest(&config, &project)?;
    let lib_rs = render_generated_lib(&config);

    write_if_changed(&generated_root.join("Cargo.toml"), &cargo_toml)?;
    write_if_changed(&generated_src.join("lib.rs"), &lib_rs)?;

    Ok(config)
}

fn cargo_command(
    project: &Path,
    config: &ScarletConfig,
    subcommand: &str,
    target: Option<String>,
    release: bool,
    extra_args: &[String],
) -> Result<(), String> {
    let resolved_target = target.unwrap_or_else(|| config.board.target_json.clone());

    metadata_check(project, &resolved_target)?;

    let mut command = Command::new("cargo");
    command.arg(subcommand);
    if release {
        command.arg("--release");
    }

    command.arg("--target").arg(resolved_target);

    if subcommand == "clippy" && !extra_args.iter().any(|arg| arg == "--") {
        command.arg("--");
        command.arg("-D");
        command.arg("warnings");
    }

    command.args(extra_args);
    command.current_dir(project);

    eprintln!(
        "cargo-scarlet: running in {} -> cargo {}",
        project.display(),
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    );

    let status = command
        .status()
        .map_err(|error| format!("failed to run cargo {subcommand}: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {subcommand} failed with status {status}"))
    }
}

fn ensure_init_target_is_safe(project: &Path) -> Result<(), String> {
    if !project.exists() {
        return Ok(());
    }

    let mut entries = fs::read_dir(project)
        .map_err(|error| format!("failed to inspect {}: {error}", project.display()))?;

    if entries.next().is_some() {
        return Err(format!(
            "refusing to initialize into non-empty directory {}",
            project.display()
        ));
    }

    Ok(())
}

fn render_init_cargo_toml(board: &str) -> String {
    let package_name = format!("{board}-bsp");
    format!(
        "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[[bin]]\nname = \"scarlet\"\npath = \"src/main.rs\"\n\n[dependencies]\nscarlet_modules = {{ package = \"scarlet-modules\", path = \".scarlet/scarlet-modules\" }}\n"
    )
}

fn render_init_main_rs(board: &str) -> Result<String, String> {
    match board {
        "riscv64-limine" => Ok(String::from(
            "#![no_std]\n#![no_main]\n\nextern crate scarlet_modules;\n\n#[unsafe(link_section = \".init\")]\n#[unsafe(no_mangle)]\npub extern \"C\" fn arch_start_kernel() -> ! {\n    scarlet_modules::force_link();\n    scarlet_modules::scarlet::arch::riscv64::boot::limine::limine_entry()\n}\n",
        )),
        "aarch64-limine" => Ok(String::from(
            "#![no_std]\n#![no_main]\n\nuse core::arch::naked_asm;\nuse scarlet_modules::scarlet::{environment::STACK_SIZE, start_ap};\n\nextern crate scarlet_modules;\n\n#[unsafe(link_section = \".init\")]\n#[unsafe(no_mangle)]\npub extern \"C\" fn arch_start_kernel() -> ! {\n    scarlet_modules::force_link();\n    scarlet_modules::scarlet::arch::aarch64::boot::limine::limine_entry()\n}\n\n#[unsafe(link_section = \".init\")]\n#[unsafe(export_name = \"_entry_ap\")]\n#[unsafe(naked)]\npub extern \"C\" fn _entry_ap() {\n    unsafe {\n        naked_asm!(\n            \"mrs x4, MPIDR_EL1\",\n            \"and x4, x4, #0xFF\",\n            \"mov x2, {stack_size}\",\n            \"adrp x3, KERNEL_STACK\",\n            \"add x3, x3, :lo12:KERNEL_STACK\",\n            \"add x5, x4, #1\",\n            \"mul x5, x5, x2\",\n            \"add x5, x3, x5\",\n            \"and sp, x5, #~0xF\",\n            \"mov x0, x4\",\n            \"bl {start_ap}\",\n            \"1:\",\n            \"wfi\",\n            \"b 1b\",\n            stack_size = const STACK_SIZE,\n            start_ap = sym start_ap,\n        );\n    }\n}\n",
        )),
        _ => Err(format!("unsupported board `{board}` for init template")),
    }
}

fn render_init_config(board: &str) -> Result<String, String> {
    let (target, target_json) = match board {
        "riscv64-limine" => (
            "riscv64gc-unknown-none-elf",
            "../../kernel/targets/riscv64gc-unknown-none-elf.json",
        ),
        "aarch64-limine" => (
            "aarch64-unknown-none-elf",
            "../../kernel/targets/aarch64-unknown-none-elf.json",
        ),
        _ => return Err(format!("unsupported board `{board}` for init template")),
    };

    Ok(format!(
        "config_version = 1\n\n[project]\nname = \"scarlet-{board}\"\n\n[board]\nname = \"{board}\"\ntarget = \"{target}\"\ntarget_json = \"{target_json}\"\n\n[kernel]\npackage = \"scarlet\"\nsource = {{ path = \"../../kernel\" }}\n\n[kernel.features]\nnetwork = true\nuser-fpu = true\nuser-vector = true\nhypervisor = true\nlimine = true\nprofiler = false\n\n[modules]\n\"scarlet-module-prototype\" = {{ path = \"../../modules/scarlet-module-prototype\", enabled = true }}\n"
    ))
}

fn normalize_project_path(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("failed to resolve {}: {error}", path.display()))
}

fn validate_config(config: &ScarletConfig) -> Result<(), String> {
    if config.config_version != 1 {
        return Err(format!(
            "unsupported config_version {} (expected 1)",
            config.config_version
        ));
    }

    if config.project.name.trim().is_empty() {
        return Err("project.name must not be empty".to_string());
    }

    if config.board.name.trim().is_empty() {
        return Err("board.name must not be empty".to_string());
    }

    if config.board.target.trim().is_empty() {
        return Err("board.target must not be empty".to_string());
    }

    if config.board.target_json.trim().is_empty() {
        return Err("board.target_json must not be empty".to_string());
    }

    if config.kernel.package.trim().is_empty() {
        return Err("kernel.package must not be empty".to_string());
    }

    validate_kernel_source(&config.kernel.source)?;

    for feature_name in config.kernel.features.keys() {
        if feature_name.trim().is_empty() {
            return Err("kernel.features keys must not be empty".to_string());
        }
    }

    for (name, module) in &config.modules {
        validate_module(name, module)?;
    }

    Ok(())
}

fn validate_kernel_source(source: &KernelSource) -> Result<(), String> {
    let mut forms = 0;
    if source.version.is_some() {
        forms += 1;
    }
    if source.path.is_some() {
        forms += 1;
    }
    if source.git.is_some() {
        forms += 1;
    }
    if forms != 1 {
        return Err("kernel.source must use exactly one of version, path, or git".to_string());
    }

    if source.git.is_some()
        && source.rev.is_none()
        && source.branch.is_none()
        && source.tag.is_none()
    {
        return Err("kernel.source git entries must include rev, branch, or tag".to_string());
    }

    if source.registry.is_some() && source.version.is_none() {
        return Err("kernel.source registry may only be used with version".to_string());
    }

    Ok(())
}

fn validate_module(name: &str, module: &ModuleConfig) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("module names must not be empty".to_string());
    }

    let mut forms = 0;
    if module.version.is_some() {
        forms += 1;
    }
    if module.path.is_some() {
        forms += 1;
    }
    if module.git.is_some() {
        forms += 1;
    }

    if forms != 1 {
        return Err(format!(
            "module `{name}` must use exactly one of version, path, or git"
        ));
    }

    if module.git.is_some()
        && module.rev.is_none()
        && module.branch.is_none()
        && module.tag.is_none()
    {
        return Err(format!(
            "module `{name}` with git source must include rev, branch, or tag"
        ));
    }

    Ok(())
}

fn render_generated_manifest(
    config: &ScarletConfig,
    project_root: &Path,
) -> Result<String, String> {
    let mut manifest = String::new();
    let fingerprint = config_fingerprint(config);
    writeln!(&mut manifest, "# generated by cargo-scarlet")
        .map_err(|error| format!("failed to render header: {error}"))?;
    writeln!(&mut manifest, "# config-fingerprint: {fingerprint}")
        .map_err(|error| format!("failed to render fingerprint: {error}"))?;
    manifest.push_str("[package]\n");
    manifest.push_str("name = \"scarlet-modules\"\n");
    manifest.push_str("version = \"0.1.0\"\n");
    manifest.push_str("edition = \"2024\"\n\n");
    manifest.push_str("[lib]\npath = \"src/lib.rs\"\n\n");
    manifest.push_str("[dependencies]\n");

    let kernel_spec = render_kernel_dependency_spec(project_root, &config.kernel)?;
    writeln!(
        &mut manifest,
        "scarlet = {{ {kernel_spec}, default-features = false, features = [{}] }}",
        render_enabled_kernel_features(&config.kernel.features)
    )
    .map_err(|error| format!("failed to render kernel dependency: {error}"))?;

    for (name, module) in &config.modules {
        if !module.enabled {
            continue;
        }

        let dependency_name = module.package.as_deref().unwrap_or(name);
        let spec = render_dependency_spec(project_root, module)?;
        writeln!(&mut manifest, "{name} = {{ {spec} }}")
            .map_err(|error| format!("failed to render dependency {dependency_name}: {error}"))?;
    }

    Ok(manifest)
}

fn render_kernel_dependency_spec(
    project_root: &Path,
    kernel: &KernelConfig,
) -> Result<String, String> {
    let mut parts = Vec::new();

    if let Some(version) = &kernel.source.version {
        parts.push(format!("version = \"{version}\""));
        if let Some(registry) = &kernel.source.registry {
            parts.push(format!("registry = \"{registry}\""));
        }
    }

    if let Some(path) = &kernel.source.path {
        let absolute = project_root.join(path);
        let generated_root = project_root.join(".scarlet/scarlet-modules");
        let relative = pathdiff(&absolute, &generated_root)?;
        parts.push(format!("path = \"{}\"", relative.display()));
    }

    if let Some(git) = &kernel.source.git {
        parts.push(format!("git = \"{git}\""));
    }
    if let Some(rev) = &kernel.source.rev {
        parts.push(format!("rev = \"{rev}\""));
    }
    if let Some(branch) = &kernel.source.branch {
        parts.push(format!("branch = \"{branch}\""));
    }
    if let Some(tag) = &kernel.source.tag {
        parts.push(format!("tag = \"{tag}\""));
    }

    Ok(parts.join(", "))
}

fn render_enabled_kernel_features(features: &BTreeMap<String, bool>) -> String {
    features
        .iter()
        .filter(|(_, enabled)| **enabled)
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_dependency_spec(project_root: &Path, module: &ModuleConfig) -> Result<String, String> {
    let mut parts = Vec::new();

    if let Some(version) = &module.version {
        parts.push(format!("version = \"{version}\""));
        if let Some(registry) = &module.registry {
            parts.push(format!("registry = \"{registry}\""));
        }
    }

    if let Some(path) = &module.path {
        let absolute = project_root.join(path);
        let generated_root = project_root.join(".scarlet/scarlet-modules");
        let relative = pathdiff(&absolute, &generated_root)?;
        parts.push(format!("path = \"{}\"", relative.display()));
    }

    if let Some(git) = &module.git {
        parts.push(format!("git = \"{git}\""));
    }
    if let Some(rev) = &module.rev {
        parts.push(format!("rev = \"{rev}\""));
    }
    if let Some(branch) = &module.branch {
        parts.push(format!("branch = \"{branch}\""));
    }
    if let Some(tag) = &module.tag {
        parts.push(format!("tag = \"{tag}\""));
    }
    if let Some(package) = &module.package {
        parts.push(format!("package = \"{package}\""));
    }
    if let Some(features) = &module.features {
        let rendered = features
            .iter()
            .map(|feature| format!("\"{feature}\""))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("features = [{rendered}]"));
    }
    if let Some(default_features) = module.default_features {
        parts.push(format!("default-features = {default_features}"));
    }

    Ok(parts.join(", "))
}

fn render_generated_lib(config: &ScarletConfig) -> String {
    let mut source = String::new();
    let _ = writeln!(&mut source, "#![no_std]");
    let _ = writeln!(&mut source);
    let _ = writeln!(
        &mut source,
        "// config-fingerprint: {}",
        config_fingerprint(config)
    );
    let _ = writeln!(&mut source, "pub use scarlet;");
    let _ = writeln!(&mut source);
    let _ = writeln!(&mut source, "#[inline(never)]");
    let _ = writeln!(&mut source, "pub fn force_link() {{");

    for name in config
        .modules
        .keys()
        .filter(|name| config.modules[*name].enabled)
    {
        let identifier = cargo_key_to_rust_identifier(name);
        let _ = writeln!(&mut source, "    {identifier}::force_link();");
    }

    source.push_str("}\n");
    source
}

fn write_if_changed(path: &Path, contents: &str) -> Result<(), String> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == contents {
            return Ok(());
        }
    }

    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn config_fingerprint(config: &ScarletConfig) -> String {
    let mut fingerprint = String::new();
    let _ = write!(
        &mut fingerprint,
        "v{}:{}:{}:{}:{}:{}",
        config.config_version,
        config.project.name,
        config.board.name,
        config.board.target,
        config.board.target_json,
        config.kernel.package,
    );

    for (name, feature_enabled) in &config.kernel.features {
        let _ = write!(&mut fingerprint, ";kf:{name}={feature_enabled}");
    }

    for (name, module) in &config.modules {
        let _ = write!(&mut fingerprint, ";m:{name}:{}", module.enabled);
        if let Some(version) = &module.version {
            let _ = write!(&mut fingerprint, ":version={version}");
        }
        if let Some(path) = &module.path {
            let _ = write!(&mut fingerprint, ":path={path}");
        }
        if let Some(git) = &module.git {
            let _ = write!(&mut fingerprint, ":git={git}");
        }
        if let Some(rev) = &module.rev {
            let _ = write!(&mut fingerprint, ":rev={rev}");
        }
        if let Some(branch) = &module.branch {
            let _ = write!(&mut fingerprint, ":branch={branch}");
        }
        if let Some(tag) = &module.tag {
            let _ = write!(&mut fingerprint, ":tag={tag}");
        }
        if let Some(registry) = &module.registry {
            let _ = write!(&mut fingerprint, ":registry={registry}");
        }
    }

    fingerprint
}

fn metadata_check(project: &Path, target: &str) -> Result<(), String> {
    let mut command = Command::new("cargo");
    command
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--filter-platform")
        .arg(target)
        .current_dir(project);

    eprintln!(
        "cargo-scarlet: running in {} -> cargo {}",
        project.display(),
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    );

    let status = command
        .status()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo metadata failed with status {status}"))
    }
}

fn cargo_key_to_rust_identifier(name: &str) -> String {
    name.replace('-', "_")
}

fn pathdiff(path: &Path, base: &Path) -> Result<PathBuf, String> {
    let path_components = path.components().collect::<Vec<_>>();
    let base_components = base.components().collect::<Vec<_>>();

    let mut common = 0usize;
    while common < path_components.len()
        && common < base_components.len()
        && path_components[common] == base_components[common]
    {
        common += 1;
    }

    let mut result = PathBuf::new();
    for _ in common..base_components.len() {
        result.push("..");
    }
    for component in &path_components[common..] {
        result.push(component.as_os_str());
    }

    if result.as_os_str().is_empty() {
        result.push(".");
    }

    Ok(result)
}
