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
    Build {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        module: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Check {
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
    New {
        #[arg(long)]
        module: Option<String>,
        #[arg(long)]
        bsp: Option<String>,
        #[arg(long)]
        kernel_path: Option<PathBuf>,
        #[arg(long)]
        kernel_rev: Option<String>,
        #[arg(long)]
        target: Option<String>,
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
    let cli = Cli::parse_from(normalized_args());
    match cli.command {
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
            module,
            output,
        } => {
            if let Some(module_path) = module {
                build_loadable_module(&module_path, target.as_deref(), output.as_deref(), release)?;
                Ok(())
            } else {
                let project = project.ok_or("--project is required when not using --module")?;
                let config = generate(&project)?;
                cargo_command(&project, &config, "build", target, release, &[])
            }
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
        Commands::New {
            module,
            bsp,
            kernel_path,
            kernel_rev,
            target,
        } => new_scaffold(
            module,
            bsp,
            kernel_path.as_deref(),
            kernel_rev.as_deref(),
            target.as_deref(),
        ),
    }
}

fn normalized_args() -> Vec<String> {
    let mut args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|arg| arg == "scarlet") {
        args.remove(1);
    }
    args
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
    if let Ok(existing) = fs::read_to_string(path)
        && existing == contents
    {
        return Ok(());
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

    if let Some(version) = &config.kernel.source.version {
        let _ = write!(&mut fingerprint, ";ks:version={version}");
    }
    if let Some(path) = &config.kernel.source.path {
        let _ = write!(&mut fingerprint, ";ks:path={path}");
    }
    if let Some(git) = &config.kernel.source.git {
        let _ = write!(&mut fingerprint, ";ks:git={git}");
    }
    if let Some(rev) = &config.kernel.source.rev {
        let _ = write!(&mut fingerprint, ";ks:rev={rev}");
    }
    if let Some(branch) = &config.kernel.source.branch {
        let _ = write!(&mut fingerprint, ";ks:branch={branch}");
    }
    if let Some(tag) = &config.kernel.source.tag {
        let _ = write!(&mut fingerprint, ";ks:tag={tag}");
    }
    if let Some(registry) = &config.kernel.source.registry {
        let _ = write!(&mut fingerprint, ";ks:registry={registry}");
    }

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

fn build_loadable_module(
    module_path: &Path,
    target: Option<&str>,
    output: Option<&Path>,
    release: bool,
) -> Result<(), String> {
    let target = target.ok_or("--target is required when using --module")?;
    let module_dir = fs::canonicalize(module_path).map_err(|e| {
        format!(
            "failed to resolve module path {}: {e}",
            module_path.display()
        )
    })?;

    let module_name = read_module_toml_name(&module_dir).ok_or_else(|| {
        format!(
            "failed to read module name from module.toml in {}",
            module_dir.display()
        )
    })?;

    let target_path = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        std::env::current_dir()
            .map_err(|e| format!("failed to get current directory: {e}"))?
            .join(target)
    };
    let target_path = fs::canonicalize(&target_path).map_err(|e| {
        format!(
            "failed to resolve target path {}: {e}",
            target_path.display()
        )
    })?;

    let target_triple = target_path
        .file_stem()
        .ok_or("target path has no file stem")?
        .to_string_lossy()
        .to_string();

    eprintln!(
        "cargo-scarlet: building loadable module {} (target: {})",
        module_dir.display(),
        target_path.display()
    );

    let mut command = Command::new("cargo");
    command.arg("rustc").arg("--target").arg(&target_path);
    if release {
        command.arg("--release");
    }
    command.arg("--").arg("--emit=obj").current_dir(&module_dir);

    let status = command
        .status()
        .map_err(|e| format!("failed to run cargo rustc: {e}"))?;

    if !status.success() {
        return Err(format!("cargo rustc failed with status {status}"));
    }

    let profile = if release { "release" } else { "debug" };
    let output_dir = module_dir.join("target").join(&target_triple).join(profile);
    let deps_dir = output_dir.join("deps");
    let lsm_filename = format!("{}.lsm", module_name);

    let mut object_files: Vec<std::path::PathBuf> = Vec::new();
    for entry in fs::read_dir(&deps_dir)
        .map_err(|e| format!("failed to read {}: {e}", deps_dir.display()))?
    {
        let entry = entry.map_err(|e| format!("failed to read dir entry: {e}"))?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "o") {
            object_files.push(path);
        }
    }

    let selected_object = if object_files.is_empty() {
        None
    } else if object_files.len() == 1 {
        Some(object_files.remove(0))
    } else {
        let normalized = cargo_key_to_rust_identifier(&module_name);
        let mut candidates: Vec<_> = object_files
            .iter()
            .filter(|path| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|stem| stem.starts_with(&normalized) || stem.contains(&normalized))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        if candidates.is_empty() {
            return Err(format!(
                "multiple .o files in {}, but none match module name '{}'",
                deps_dir.display(),
                module_name
            ));
        }

        candidates.sort_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        candidates.pop()
    };

    let mut built = false;
    if let Some(object_path) = selected_object {
        let lsm_path = output_dir.join(&lsm_filename);
        fs::rename(&object_path, &lsm_path)
            .map_err(|e| format!("failed to rename object file to .lsm: {e}"))?;
        eprintln!("cargo-scarlet: produced {}", lsm_path.display());
        built = true;
    }

    if !built {
        for entry in fs::read_dir(&output_dir)
            .map_err(|e| format!("failed to read {}: {e}", output_dir.display()))?
        {
            let entry = entry.map_err(|e| format!("failed to read dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "lsm") {
                built = true;
                break;
            }
        }
    }

    if !built {
        return Err("no .o files produced by cargo rustc".to_string());
    }

    if let Some(output) = output {
        let output_dir = std::env::current_dir()
            .map_err(|e| format!("failed to get current directory: {e}"))?
            .join(output);
        fs::create_dir_all(&output_dir).map_err(|e| format!("failed to create output dir: {e}"))?;
        let lsm_path = module_dir
            .join("target")
            .join(&target_triple)
            .join(profile)
            .join(&lsm_filename);
        let dest = output_dir.join(&lsm_filename);
        fs::copy(&lsm_path, &dest).map_err(|e| format!("failed to copy .lsm to output: {e}"))?;
        eprintln!("cargo-scarlet: copied to {}", dest.display());
    }

    Ok(())
}

fn read_module_toml_name(module_dir: &Path) -> Option<String> {
    let content = fs::read_to_string(module_dir.join("module.toml")).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("name")
            && let Some(eq_pos) = trimmed.find('=')
            && let Some(value) = trimmed.get(eq_pos + 1..).map(str::trim)
            && value.starts_with('"')
            && value.ends_with('"')
            && value.len() >= 2
        {
            return Some(value[1..value.len() - 1].to_string());
        }
    }
    None
}

const KERNEL_GIT_URL: &str = "https://github.com/petitstrawberry/Scarlet";
const KERNEL_DEFAULT_REV: &str = "v0.17.0";

fn new_scaffold(
    module: Option<String>,
    bsp: Option<String>,
    kernel_path: Option<&Path>,
    kernel_rev: Option<&str>,
    target: Option<&str>,
) -> Result<(), String> {
    match (module, bsp) {
        (Some(name), None) => scaffold_module(&name, kernel_path, kernel_rev),
        (None, Some(name)) => scaffold_bsp(&name, kernel_path, kernel_rev, target),
        (Some(_), Some(_)) => Err("cannot specify both --module and --bsp".to_string()),
        (None, None) => Err("specify --module or --bsp".to_string()),
    }
}

fn kernel_dependency_spec(
    kernel_path: Option<&Path>,
    kernel_rev: Option<&str>,
    module_dir: &Path,
) -> Result<String, String> {
    if let Some(path) = kernel_path {
        let abs_kernel = fs::canonicalize(path).map_err(|e| format!("{e}: {}", path.display()))?;
        let abs_module = module_dir.to_path_buf();
        let rel = pathdiff(&abs_kernel, &abs_module)?;
        Ok(format!("path = \"{}\"", rel.display()))
    } else {
        let rev = kernel_rev.unwrap_or(KERNEL_DEFAULT_REV);
        Ok(format!("git = \"{KERNEL_GIT_URL}\", rev = \"{rev}\""))
    }
}

fn kernel_source_toml(
    kernel_path: Option<&Path>,
    kernel_rev: Option<&str>,
    base_dir: &Path,
) -> Result<String, String> {
    if let Some(path) = kernel_path {
        let abs_kernel = fs::canonicalize(path).map_err(|e| format!("{e}: {}", path.display()))?;
        let rel = pathdiff(&abs_kernel, base_dir)?;
        Ok(format!("{{ path = \"{}\" }}", rel.display()))
    } else {
        let rev = kernel_rev.unwrap_or(KERNEL_DEFAULT_REV);
        Ok(format!("{{ git = \"{KERNEL_GIT_URL}\", rev = \"{rev}\" }}"))
    }
}

fn scaffold_module(
    name: &str,
    kernel_path: Option<&Path>,
    kernel_rev: Option<&str>,
) -> Result<(), String> {
    let module_dir = PathBuf::from(name);
    let kernel_spec = kernel_dependency_spec(kernel_path, kernel_rev, &module_dir)?;
    let crate_name = cargo_key_to_rust_identifier(name);
    let src_dir = module_dir.join("src");
    let cargo_dir = module_dir.join(".cargo");

    fs::create_dir_all(&src_dir)
        .map_err(|e| format!("failed to create {}: {e}", src_dir.display()))?;
    fs::create_dir_all(&cargo_dir)
        .map_err(|e| format!("failed to create {}: {e}", cargo_dir.display()))?;

    let name_bytes = name.as_bytes();
    let name_with_null_len = name_bytes.len() + 1;

    let cargo_toml = format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
scarlet = {{ {kernel_spec} }}
"#
    );
    write_if_changed(&module_dir.join("Cargo.toml"), &cargo_toml)?;

    let module_toml = format!(
        r#"[module]
name = "{name}"
depends = []
"#
    );
    write_if_changed(&module_dir.join("module.toml"), &module_toml)?;

    let build_rs = r#"use std::path::Path;

fn parse_depends(content: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut rest = content;

    while let Some(depends_pos) = rest.find("depends") {
        rest = &rest[depends_pos + "depends".len()..];
        let Some(eq_pos) = rest.find('=') else {
            break;
        };
        rest = &rest[eq_pos + 1..];
        let Some(open_pos) = rest.find('[') else {
            break;
        };
        rest = &rest[open_pos + 1..];
        let Some(close_pos) = rest.find(']') else {
            break;
        };

        let array = &rest[..close_pos];
        for item in array.split(',') {
            let trimmed = item.trim();
            if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
                deps.push(trimmed[1..trimmed.len() - 1].to_string());
            }
        }
        break;
    }

    deps
}

fn main() {
    let rustc_version = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = std::process::Command::new(rustc_version)
        .arg("--version")
        .output()
        .expect("failed to run rustc --version");
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!("cargo:rustc-env=RUSTC_VERSION={version}");

    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=TARGET={target}");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let module_toml = Path::new(&manifest_dir).join("module.toml");
    println!("cargo:rerun-if-changed={}", module_toml.display());

    if module_toml.exists() {
        let content = std::fs::read_to_string(&module_toml).expect("failed to read module.toml");
        let depends = parse_depends(&content);
        println!("cargo:rustc-env=SCARLET_LSM_DEPENDS={}", depends.join(","));
    } else {
        println!("cargo:rustc-env=SCARLET_LSM_DEPENDS=");
    }
}
"#;
    write_if_changed(&module_dir.join("build.rs"), build_rs)?;

    let lib_rs = format!(
        r#"#![no_std]

use scarlet::early_println;

#[unsafe(no_mangle)]
pub static SCARLET_LSM_NAME: [u8; {name_with_null_len}] = *b"{name}\0";

#[unsafe(no_mangle)]
pub static SCARLET_LSM_BUILD_INFO: [u8; 72] = {{
    let s = concat!(env!("RUSTC_VERSION"), ";", env!("TARGET"), "\0");
    let bytes: &[u8] = s.as_bytes();
    let mut arr = [0u8; 72];
    let mut i = 0;
    while i < bytes.len() && i < 72 {{
        arr[i] = bytes[i];
        i += 1;
    }}
    arr
}};

#[unsafe(no_mangle)]
pub static SCARLET_LSM_DEPENDS: [u8; 256] = {{
    let s = concat!(env!("SCARLET_LSM_DEPENDS"), "\0");
    let bytes: &[u8] = s.as_bytes();
    let mut arr = [0u8; 256];
    let mut i = 0;
    while i < bytes.len() && i < 256 {{
        arr[i] = bytes[i];
        i += 1;
    }}
    arr
}};

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_lsm_init() -> Result<(), &'static str> {{
    early_println!("[{name}] loaded!");
    Ok(())
}}
"#
    );
    write_if_changed(&src_dir.join("lib.rs"), &lib_rs)?;

    let cargo_config = r#"[target.riscv64gc-unknown-none-elf]
runner = "true"

[target.aarch64-unknown-none-elf]
runner = "true"

[profile.dev]
opt-level = 3

[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler-builtins-mem"]
unstable-options = true
"#;
    write_if_changed(&cargo_dir.join("config.toml"), cargo_config)?;

    let _ = write_if_changed(&module_dir.join(".gitignore"), "target/\n");

    eprintln!("cargo-scarlet: created loadable module '{name}'");
    Ok(())
}

fn render_bsp_build_rs(kernel_symbols_relative: &str) -> String {
    let const_line = format!(
        "const KERNEL_SYMBOLS_RELATIVE: &str = \"{}\";",
        kernel_symbols_relative
    );

    format!(
        r##"use std::path::Path;
use std::process::Command;

{const_line}

fn main() {{
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = std::env::var("TARGET").unwrap();
    let profile = std::env::var("PROFILE").unwrap();
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| format!("{{manifest_dir}}/target"));

    let binary_path = format!("{{target_dir}}/{{target}}/{{profile}}/scarlet");
    let kernel_symbols_path = format!("{{manifest_dir}}/{{KERNEL_SYMBOLS_RELATIVE}}");

    if Path::new(&binary_path).exists() {{
        extract_symbols(&binary_path, &kernel_symbols_path);
    }} else {{
        generate_empty_symbols(&kernel_symbols_path);
    }}
}}

fn extract_symbols(binary_path: &str, output_path: &str) {{
    let output = Command::new("nm")
        .args(["--defined-only", "--extern-only", "-g", "--no-sort", binary_path])
        .output()
        .expect("failed to run nm");

    if !output.status.success() {{
        eprintln!("cargo-scarlet [build.rs]: nm failed, generating empty symbols");
        generate_empty_symbols(output_path);
        return;
    }}

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut symbols: Vec<(String, String)> = Vec::new();

    for line in stdout.lines() {{
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {{
            continue;
        }}
        let addr = parts[0];
        let name = parts[2];

        if name.is_empty() {{
            continue;
        }}

        let skip = match name {{
            "_GLOBAL_OFFSET_TABLE_" | "_DYNAMIC" => true,
            _ if name.starts_with("__") && name.ends_with("_START") => true,
            _ if name.starts_with("__") && name.ends_with("_END") => true,
            _ => false,
        }};

        if !skip {{
            symbols.push((name.to_string(), addr.to_string()));
        }}
    }}

    let count = symbols.len();
    let mut content = String::new();
    content.push_str("#[allow(dead_code)]\n\n");
    content.push_str("#[unsafe(link_section = \".lsm_symbols\")]\n");
    content.push_str("#[used]\n");
    content.push_str("static _FORCE_SECTION: usize = 0;\n\n");
    content.push_str("#[allow(dead_code)]\n");
    content.push_str(&format!(
        "static KERNEL_SYMBOLS: [(&'static str, usize); {{count}}] = [\n"
    ));
    for (name, addr) in &symbols {{
        content.push_str(&format!("    (\"{{name}}\", 0x{{addr}}),\n"));
    }}
    content.push_str("];\n\n");
    content.push_str("pub fn get_kernel_symbols() -> &'static [(&'static str, usize)] {{ &KERNEL_SYMBOLS }}\n");

    write_if_changed(output_path, &content);
    eprintln!(
        "cargo-scarlet [build.rs]: extracted {{count}} kernel symbols from {{}}",
        binary_path
    );
}}

fn generate_empty_symbols(output_path: &str) {{
    let content = "#[allow(dead_code)]\n\n\
        #[unsafe(link_section = \".lsm_symbols\")]\n\
        #[used]\n\
        static _FORCE_SECTION: usize = 0;\n\n\
        #[allow(dead_code)]\n\
        static KERNEL_SYMBOLS: [(&'static str, usize); 0] = [];\n\n\
        pub fn get_kernel_symbols() -> &'static [(&'static str, usize)] {{ &KERNEL_SYMBOLS }}\n";
    write_if_changed(output_path, content);
}}

fn write_if_changed(path: &str, contents: &str) {{
    if let Ok(existing) = std::fs::read_to_string(path) {{
        if existing == contents {{
            return;
        }}
    }}
    std::fs::write(path, contents).expect("failed to write generated_symbols.rs");
}}
"##
    )
}

fn scaffold_bsp(
    name: &str,
    kernel_path: Option<&Path>,
    kernel_rev: Option<&str>,
    target: Option<&str>,
) -> Result<(), String> {
    let target = target.ok_or("--target is required for BSP")?;
    let bsp_dir = PathBuf::from(name);
    let kernel_spec = kernel_dependency_spec(kernel_path, kernel_rev, &bsp_dir)?;
    let kernel_source = kernel_source_toml(kernel_path, kernel_rev, &bsp_dir)?;
    let target_json_dir = match kernel_path {
        Some(p) => {
            let abs = fs::canonicalize(p).map_err(|e| format!("{e}: {}", p.display()))?;
            let rel = pathdiff(&abs, &bsp_dir)?;
            format!("{}/targets/{}", rel.display(), target)
        }
        None => format!("../../kernel/targets/{target}"),
    };
    let src_dir = bsp_dir.join("src");
    let lds_dir = bsp_dir.join("lds");
    let cargo_dir = bsp_dir.join(".cargo");
    let scarlet_modules_dir = bsp_dir.join(".scarlet/scarlet-modules/src");

    let kernel_symbols_relative = match kernel_path {
        Some(p) => {
            let abs_kernel = fs::canonicalize(p).map_err(|e| format!("{e}: {}", p.display()))?;
            let abs_symbols = abs_kernel.join("src/lsm/generated_symbols.rs");
            let abs_bsp = std::env::current_dir()
                .map_err(|e| format!("failed to get cwd: {e}"))?
                .join(&bsp_dir);
            pathdiff(&abs_symbols, &abs_bsp)?.display().to_string()
        }
        None => String::new(),
    };

    fs::create_dir_all(&src_dir)
        .map_err(|e| format!("failed to create {}: {e}", src_dir.display()))?;
    fs::create_dir_all(&lds_dir)
        .map_err(|e| format!("failed to create {}: {e}", lds_dir.display()))?;
    fs::create_dir_all(&cargo_dir)
        .map_err(|e| format!("failed to create {}: {e}", cargo_dir.display()))?;
    fs::create_dir_all(&scarlet_modules_dir)
        .map_err(|e| format!("failed to create {}: {e}", scarlet_modules_dir.display()))?;

    let crate_name = cargo_key_to_rust_identifier(name);

    let build_rs = render_bsp_build_rs(&kernel_symbols_relative);
    write_if_changed(&bsp_dir.join("build.rs"), &build_rs)?;
    let main_rs = r#"#![no_std]
#![no_main]

extern crate scarlet_modules;

use scarlet_modules::scarlet;

#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel() -> ! {{
    scarlet_modules::force_link();
    // REQUIRED: implement architecture-specific boot entry
    // e.g. scarlet_modules::scarlet::arch::riscv64::boot::limine::limine_entry()
    loop {{}}
}}
"#
    .to_string();
    write_if_changed(&src_dir.join("main.rs"), &main_rs)?;

    let bsp_cargo_toml = format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "scarlet"
path = "src/main.rs"

[dependencies]
scarlet_modules = {{ package = "scarlet-modules", path = ".scarlet/scarlet-modules" }}
"#
    );
    write_if_changed(&bsp_dir.join("Cargo.toml"), &bsp_cargo_toml)?;

    let scarlet_config = format!(
        r#"config_version = 1

[project]
name = "scarlet-{name}"

[board]
name = "{name}"
target = "{target}"
target_json = "{target_json_dir}"

[kernel]
package = "scarlet"
source = {kernel_source}

[kernel.features]

[modules]
"#
    );
    write_if_changed(&bsp_dir.join("scarlet-config.toml"), &scarlet_config)?;

    let cargo_config = format!(
        r#"[profile.dev]
opt-level = 3

[profile.test]
opt-level = 3

[build]
target = "{target_json_dir}"

[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler-builtins-mem"]
unstable-options = true
"#
    );
    write_if_changed(&cargo_dir.join("config.toml"), &cargo_config)?;

    let modules_cargo_toml = format!(
        r#"# generated by cargo-scarlet

[package]
name = "scarlet-modules"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
scarlet = {{ {kernel_spec}, default-features = false }}
"#
    );
    write_if_changed(
        &bsp_dir.join(".scarlet/scarlet-modules/Cargo.toml"),
        &modules_cargo_toml,
    )?;

    let modules_lib_rs = r#"#![no_std]

pub use scarlet;

#[inline(never)]
pub fn force_link() {}
"#;
    write_if_changed(
        &bsp_dir.join(".scarlet/scarlet-modules/src/lib.rs"),
        modules_lib_rs,
    )?;

    let _ = write_if_changed(&bsp_dir.join(".gitignore"), ".scarlet\ntarget\n");

    eprintln!("cargo-scarlet: created BSP '{name}'");
    eprintln!("cargo-scarlet: REQUIRED: update .cargo/config.toml with runner");
    eprintln!("cargo-scarlet: REQUIRED: add linker script to lds/");
    eprintln!("cargo-scarlet: REQUIRED: implement boot entry in src/main.rs (arch_start_kernel)");

    Ok(())
}
