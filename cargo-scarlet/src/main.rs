use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
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
    Image {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        kernel_elf: Option<PathBuf>,
        #[arg(long)]
        no_build: bool,
    },
    New {
        #[arg(long)]
        module: Option<String>,
        #[arg(long)]
        project: Option<String>,
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
    #[serde(rename = "loadable_modules", default)]
    loadable_modules: BTreeMap<String, LoadableModuleConfig>,
    #[serde(default)]
    image: Option<ImageConfig>,
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

#[derive(Debug, Deserialize)]
struct LoadableModuleConfig {
    path: String,
    #[serde(default = "default_true")]
    enabled: bool,
    output: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImageConfig {
    #[serde(default)]
    steps: Vec<ImageStepConfig>,
}

#[derive(Debug, Deserialize)]
struct ImageStepConfig {
    name: Option<String>,
    kind: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    inputs: Vec<ImageInputConfig>,
    output: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImageInputConfig {
    source: String,
    destination: String,
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    skip_suffixes: Vec<String>,
}

fn default_true() -> bool {
    true
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
                cargo_command(&project, &config, "build", target.clone(), release, &[])?;
                inject_ksym_section(&project, &config, target.as_deref(), release)?;
                build_loadable_modules(&project, &config, release)
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
        Commands::Image {
            project,
            target,
            release,
            kernel_elf,
            no_build,
        } => build_project_image(&project, target, release, kernel_elf, no_build),
        Commands::New {
            module,
            project,
            kernel_path,
            kernel_rev,
            target,
        } => new_scaffold(
            module,
            project,
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

    if let Some(image) = &config.image {
        for (index, step) in image.steps.iter().enumerate() {
            let has_kind = step
                .kind
                .as_deref()
                .is_some_and(|kind| !kind.trim().is_empty());
            let has_command = step
                .command
                .as_deref()
                .is_some_and(|command| !command.trim().is_empty());
            if has_kind == has_command {
                return Err(format!(
                    "image.steps[{index}] must use exactly one of kind or command"
                ));
            }
            if matches!(
                step.kind.as_deref(),
                Some("archive.newc" | "initramfs-newc")
            ) && step.output.is_none()
            {
                return Err(format!(
                    "image.steps[{index}] kind archive.newc requires output"
                ));
            }
        }
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

fn build_loadable_modules(
    project: &Path,
    config: &ScarletConfig,
    release: bool,
) -> Result<(), String> {
    if config.loadable_modules.is_empty() {
        return Ok(());
    }

    let project_dir = fs::canonicalize(project)
        .map_err(|e| format!("failed to resolve project path {}: {e}", project.display()))?;

    let target_json = &config.board.target_json;

    for (name, module) in &config.loadable_modules {
        if !module.enabled {
            eprintln!("cargo-scarlet: skipping disabled loadable module '{name}'");
            continue;
        }

        let module_path = project_dir.join(&module.path);
        let target_path = project_dir.join(target_json);
        let output_path = module.output.as_deref().map(Path::new);

        eprintln!("cargo-scarlet: building loadable module '{name}'");
        build_loadable_module(&module_path, target_path.to_str(), output_path, release)?;
    }

    Ok(())
}

fn build_project_image(
    project: &Path,
    target: Option<String>,
    release: bool,
    kernel_elf: Option<PathBuf>,
    no_build: bool,
) -> Result<(), String> {
    let project = normalize_project_path(project)?;
    let config = generate(&project)?;

    if !no_build {
        cargo_command(&project, &config, "build", target.clone(), release, &[])?;
        inject_ksym_section(&project, &config, target.as_deref(), release)?;
        build_loadable_modules(&project, &config, release)?;
    }

    let kernel_elf = match kernel_elf {
        Some(path) => absolutize_from_current_dir(&path)?,
        None => project_kernel_elf_path(&project, &config, target.as_deref(), release)?,
    };

    if !kernel_elf.exists() {
        return Err(format!("kernel ELF not found: {}", kernel_elf.display()));
    }

    let image = config
        .image
        .as_ref()
        .ok_or("project has no [image] configuration")?;
    if image.steps.is_empty() {
        return Err("project image configuration has no [[image.steps]] entries".to_string());
    }

    let images_dir = project.join(".scarlet/images");
    fs::create_dir_all(&images_dir)
        .map_err(|error| format!("failed to create {}: {error}", images_dir.display()))?;

    let target_triple = target_triple_for_project(&project, &config, target.as_deref())?;
    let profile = if release { "release" } else { "debug" };

    for (index, step) in image.steps.iter().enumerate() {
        run_image_step(
            &project,
            &config,
            step,
            index,
            &kernel_elf,
            profile,
            &target_triple,
        )?;
    }

    Ok(())
}

fn run_image_step(
    project: &Path,
    config: &ScarletConfig,
    step: &ImageStepConfig,
    index: usize,
    kernel_elf: &Path,
    profile: &str,
    target_triple: &str,
) -> Result<(), String> {
    let step_name = step
        .name
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| format!("step-{index}"));
    if let Some(kind) = step.kind.as_deref() {
        return run_builtin_image_step(
            project,
            config,
            step,
            &step_name,
            kind,
            kernel_elf,
            profile,
            target_triple,
        );
    }

    let cwd = match &step.cwd {
        Some(cwd) => {
            let rendered =
                render_image_template(cwd, project, config, kernel_elf, profile, target_triple);
            absolutize_from_base(project, Path::new(&rendered))?
        }
        None => project.to_path_buf(),
    };

    let rendered_args = step
        .args
        .iter()
        .map(|arg| render_image_template(arg, project, config, kernel_elf, profile, target_triple))
        .collect::<Vec<_>>();

    let command_program = step
        .command
        .as_deref()
        .ok_or("internal error: command image step without command")?;
    let mut command = Command::new(render_image_template(
        command_program,
        project,
        config,
        kernel_elf,
        profile,
        target_triple,
    ));
    command.args(&rendered_args).current_dir(&cwd);

    for (name, value) in &step.env {
        command.env(
            name,
            render_image_template(value, project, config, kernel_elf, profile, target_triple),
        );
    }

    eprintln!(
        "cargo-scarlet: image {step_name}: running in {} -> {} {}",
        cwd.display(),
        command.get_program().to_string_lossy(),
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    );

    let status = command
        .status()
        .map_err(|error| format!("failed to run image step {step_name}: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "image step {step_name} failed with status {status}"
        ))
    }
}

fn run_builtin_image_step(
    project: &Path,
    config: &ScarletConfig,
    step: &ImageStepConfig,
    step_name: &str,
    kind: &str,
    kernel_elf: &Path,
    profile: &str,
    target_triple: &str,
) -> Result<(), String> {
    match kind {
        "archive.newc" | "initramfs-newc" => build_initramfs_newc(
            project,
            config,
            step,
            step_name,
            kernel_elf,
            profile,
            target_triple,
        ),
        _ => Err(format!("unknown image step kind `{kind}`")),
    }
}

fn build_initramfs_newc(
    project: &Path,
    config: &ScarletConfig,
    step: &ImageStepConfig,
    step_name: &str,
    kernel_elf: &Path,
    profile: &str,
    target_triple: &str,
) -> Result<(), String> {
    let output = render_required_path_template(
        step.output.as_deref(),
        "archive.newc output",
        project,
        config,
        kernel_elf,
        profile,
        target_triple,
    )?;
    let stage_dir = project
        .join(".scarlet/initramfs-stage")
        .join(cargo_key_to_rust_identifier(step_name));
    if stage_dir.exists() {
        fs::remove_dir_all(&stage_dir)
            .map_err(|error| format!("failed to remove {}: {error}", stage_dir.display()))?;
    }
    fs::create_dir_all(&stage_dir)
        .map_err(|error| format!("failed to create {}: {error}", stage_dir.display()))?;

    for input in &step.inputs {
        let source = render_image_template(
            &input.source,
            project,
            config,
            kernel_elf,
            profile,
            target_triple,
        );
        let source = absolutize_from_base(project, Path::new(&source))?;
        if !source.exists() {
            if input.optional {
                eprintln!(
                    "cargo-scarlet: image {step_name}: skipping missing optional input {}",
                    source.display()
                );
                continue;
            }
            return Err(format!("image input not found: {}", source.display()));
        }

        let destination = normalize_archive_path(&render_image_template(
            &input.destination,
            project,
            config,
            kernel_elf,
            profile,
            target_triple,
        ))?;
        let destination = stage_dir.join(destination);
        copy_image_input(&source, &destination, &input.skip_suffixes)?;
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    write_newc_archive(&stage_dir, &output)?;
    eprintln!(
        "cargo-scarlet: image {step_name}: created newc archive {}",
        output.display()
    );
    Ok(())
}

fn render_image_template(
    value: &str,
    project: &Path,
    config: &ScarletConfig,
    kernel_elf: &Path,
    profile: &str,
    target_triple: &str,
) -> String {
    let repo = std::env::current_dir().unwrap_or_else(|_| project.to_path_buf());
    value
        .replace("{project}", &project.display().to_string())
        .replace("{repo}", &repo.display().to_string())
        .replace("{kernel_elf}", &kernel_elf.display().to_string())
        .replace("{profile}", profile)
        .replace("{target_triple}", target_triple)
        .replace("{board}", &config.board.name)
}

fn project_kernel_elf_path(
    project: &Path,
    config: &ScarletConfig,
    target: Option<&str>,
    release: bool,
) -> Result<PathBuf, String> {
    let target_triple = target_triple_for_project(project, config, target)?;
    let profile = if release { "release" } else { "debug" };
    Ok(project
        .join("target")
        .join(target_triple)
        .join(profile)
        .join("scarlet"))
}

fn target_triple_for_project(
    project: &Path,
    config: &ScarletConfig,
    target: Option<&str>,
) -> Result<String, String> {
    let resolved_target = target.unwrap_or(&config.board.target_json);
    let target_path = if Path::new(resolved_target).is_absolute() {
        PathBuf::from(resolved_target)
    } else {
        project.join(resolved_target)
    };
    Ok(target_path
        .file_stem()
        .ok_or("target path has no file stem")?
        .to_string_lossy()
        .to_string())
}

fn absolutize_from_current_dir(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| format!("failed to get current directory: {error}"))?
            .join(path))
    }
}

fn absolutize_from_base(base: &Path, path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(base.join(path))
    }
}

fn render_required_path_template(
    value: Option<&str>,
    field_name: &str,
    project: &Path,
    config: &ScarletConfig,
    kernel_elf: &Path,
    profile: &str,
    target_triple: &str,
) -> Result<PathBuf, String> {
    let value = value.ok_or_else(|| format!("{field_name} is required"))?;
    let rendered =
        render_image_template(value, project, config, kernel_elf, profile, target_triple);
    absolutize_from_base(project, Path::new(&rendered))
}

fn normalize_archive_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("archive destination must not be empty".to_string());
    }
    let trimmed = trimmed.trim_start_matches('/');
    if trimmed.is_empty() {
        Ok(PathBuf::new())
    } else {
        let path = Path::new(trimmed);
        if path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        }) {
            return Err(format!("invalid archive destination: {path:?}"));
        }
        Ok(path.to_path_buf())
    }
}

fn copy_image_input(
    source: &Path,
    destination: &Path,
    skip_suffixes: &[String],
) -> Result<(), String> {
    if source.is_dir() {
        fs::create_dir_all(destination)
            .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
        copy_dir_contents(source, destination, skip_suffixes)
    } else {
        if should_skip_path(source, skip_suffixes) {
            return Ok(());
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::copy(source, destination).map_err(|error| {
            format!(
                "failed to copy {} to {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        copy_permissions(source, destination)
    }
}

fn copy_dir_contents(
    source: &Path,
    destination: &Path,
    skip_suffixes: &[String],
) -> Result<(), String> {
    for entry in sorted_dir_entries(source)? {
        let file_name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("non-UTF-8 file name under {}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(file_name);
        if source_path.is_dir() {
            fs::create_dir_all(&destination_path).map_err(|error| {
                format!("failed to create {}: {error}", destination_path.display())
            })?;
            copy_permissions(&source_path, &destination_path)?;
            copy_dir_contents(&source_path, &destination_path, skip_suffixes)?;
        } else if source_path.is_file() && !should_skip_path(&source_path, skip_suffixes) {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
            }
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "failed to copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
            copy_permissions(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn should_skip_path(path: &Path, skip_suffixes: &[String]) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    skip_suffixes.iter().any(|suffix| name.ends_with(suffix))
}

fn copy_permissions(source: &Path, destination: &Path) -> Result<(), String> {
    let permissions = fs::metadata(source)
        .map_err(|error| format!("failed to stat {}: {error}", source.display()))?
        .permissions();
    fs::set_permissions(destination, permissions)
        .map_err(|error| format!("failed to chmod {}: {error}", destination.display()))
}

fn sorted_dir_entries(path: &Path) -> Result<Vec<fs::DirEntry>, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read dir entry in {}: {error}", path.display()))?;
    entries.sort_by_key(|entry| entry.path());
    Ok(entries)
}

fn write_newc_archive(source_root: &Path, output: &Path) -> Result<(), String> {
    let mut file = fs::File::create(output)
        .map_err(|error| format!("failed to create {}: {error}", output.display()))?;
    write_newc_entry(&mut file, ".", 0o040755, &[])?;
    write_newc_tree(&mut file, source_root, source_root)?;
    write_newc_entry(&mut file, "TRAILER!!!", 0, &[])?;
    Ok(())
}

fn write_newc_tree(output: &mut fs::File, root: &Path, path: &Path) -> Result<(), String> {
    for entry in sorted_dir_entries(path)? {
        let entry_path = entry.path();
        let relative = entry_path
            .strip_prefix(root)
            .map_err(|error| format!("failed to strip path prefix: {error}"))?;
        let name = relative
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 archive path: {}", relative.display()))?;
        let metadata = fs::symlink_metadata(&entry_path)
            .map_err(|error| format!("failed to stat {}: {error}", entry_path.display()))?;

        if metadata.is_dir() {
            write_newc_entry(output, name, 0o040000 | unix_mode(&metadata), &[])?;
            write_newc_tree(output, root, &entry_path)?;
        } else if metadata.is_file() {
            let mut contents = Vec::new();
            fs::File::open(&entry_path)
                .map_err(|error| format!("failed to open {}: {error}", entry_path.display()))?
                .read_to_end(&mut contents)
                .map_err(|error| format!("failed to read {}: {error}", entry_path.display()))?;
            write_newc_entry(output, name, 0o100000 | unix_mode(&metadata), &contents)?;
        }
    }
    Ok(())
}

fn write_newc_entry(
    output: &mut fs::File,
    name: &str,
    mode: u32,
    contents: &[u8],
) -> Result<(), String> {
    let name_size = name.len() + 1;
    let file_size = contents.len();
    let header = format!(
        "070701{ino:08x}{mode:08x}{uid:08x}{gid:08x}{nlink:08x}{mtime:08x}{file_size:08x}{dev_major:08x}{dev_minor:08x}{rdev_major:08x}{rdev_minor:08x}{name_size:08x}{check:08x}",
        ino = 0,
        mode = mode,
        uid = 0,
        gid = 0,
        nlink = 1,
        mtime = 0,
        file_size = file_size,
        dev_major = 0,
        dev_minor = 0,
        rdev_major = 0,
        rdev_minor = 0,
        name_size = name_size,
        check = 0,
    );
    output
        .write_all(header.as_bytes())
        .and_then(|_| output.write_all(name.as_bytes()))
        .and_then(|_| output.write_all(&[0]))
        .map_err(|error| format!("failed to write cpio header: {error}"))?;
    pad4(output, 110 + name_size)?;
    output
        .write_all(contents)
        .map_err(|error| format!("failed to write cpio contents: {error}"))?;
    pad4(output, file_size)?;
    Ok(())
}

fn pad4(output: &mut fs::File, size: usize) -> Result<(), String> {
    let padding = (4 - (size % 4)) % 4;
    if padding != 0 {
        output
            .write_all(&[0; 3][..padding])
            .map_err(|error| format!("failed to write cpio padding: {error}"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn unix_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn unix_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o444
    } else {
        0o755
    }
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
    let package_name = read_cargo_package_name(&module_dir);

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
        let mut normalized_names = vec![cargo_key_to_rust_identifier(&module_name)];
        if let Some(package_name) = package_name.as_deref() {
            let normalized_package_name = cargo_key_to_rust_identifier(package_name);
            if !normalized_names.contains(&normalized_package_name) {
                normalized_names.push(normalized_package_name);
            }
        }
        let candidates: Vec<_> = object_files
            .into_iter()
            .filter(|path| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|stem| normalized_names.iter().any(|name| stem.starts_with(name)))
                    .unwrap_or(false)
            })
            .collect();

        match candidates.len() {
            0 => {
                return Err(format!(
                    "multiple .o files in {}, but none match module name '{}'",
                    deps_dir.display(),
                    module_name
                ));
            }
            1 => Some(candidates.into_iter().next().unwrap()),
            _ => {
                return Err(format!(
                    "multiple .o files in {} match module name '{}'; cannot determine which to use",
                    deps_dir.display(),
                    module_name
                ));
            }
        }
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

fn read_cargo_package_name(module_dir: &Path) -> Option<String> {
    let content = fs::read_to_string(module_dir.join("Cargo.toml")).ok()?;
    let mut in_package_section = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package_section = trimmed == "[package]";
            continue;
        }

        if in_package_section
            && trimmed.starts_with("name")
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
    project: Option<String>,
    kernel_path: Option<&Path>,
    kernel_rev: Option<&str>,
    target: Option<&str>,
) -> Result<(), String> {
    match (module, project) {
        (Some(name), None) => scaffold_module(&name, kernel_path, kernel_rev),
        (None, Some(name)) => scaffold_project(&name, kernel_path, kernel_rev, target),
        (Some(_), Some(_)) => Err("cannot specify both --module and --project".to_string()),
        (None, None) => Err("specify --module or --project".to_string()),
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

fn render_project_build_rs() -> String {
    "fn main() {}\n".to_string()
}

fn scaffold_project(
    name: &str,
    kernel_path: Option<&Path>,
    kernel_rev: Option<&str>,
    target: Option<&str>,
) -> Result<(), String> {
    let target = target.ok_or("--target is required for project")?;
    let project_dir = PathBuf::from(name);
    let kernel_spec = kernel_dependency_spec(kernel_path, kernel_rev, &project_dir)?;
    let kernel_source = kernel_source_toml(kernel_path, kernel_rev, &project_dir)?;
    let target_json_dir = match kernel_path {
        Some(p) => {
            let abs = fs::canonicalize(p).map_err(|e| format!("{e}: {}", p.display()))?;
            let rel = pathdiff(&abs, &project_dir)?;
            format!("{}/targets/{}", rel.display(), target)
        }
        None => format!("../../kernel/targets/{target}"),
    };
    let src_dir = project_dir.join("src");
    let lds_dir = project_dir.join("lds");
    let cargo_dir = project_dir.join(".cargo");
    let scarlet_modules_dir = project_dir.join(".scarlet/scarlet-modules/src");

    fs::create_dir_all(&src_dir)
        .map_err(|e| format!("failed to create {}: {e}", src_dir.display()))?;
    fs::create_dir_all(&lds_dir)
        .map_err(|e| format!("failed to create {}: {e}", lds_dir.display()))?;
    fs::create_dir_all(&cargo_dir)
        .map_err(|e| format!("failed to create {}: {e}", cargo_dir.display()))?;
    fs::create_dir_all(&scarlet_modules_dir)
        .map_err(|e| format!("failed to create {}: {e}", scarlet_modules_dir.display()))?;

    let crate_name = cargo_key_to_rust_identifier(name);

    let build_rs = render_project_build_rs();
    write_if_changed(&project_dir.join("build.rs"), &build_rs)?;
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

    let project_cargo_toml = format!(
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
    write_if_changed(&project_dir.join("Cargo.toml"), &project_cargo_toml)?;

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
    write_if_changed(&project_dir.join("scarlet-config.toml"), &scarlet_config)?;

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
        &project_dir.join(".scarlet/scarlet-modules/Cargo.toml"),
        &modules_cargo_toml,
    )?;

    let modules_lib_rs = r#"#![no_std]

pub use scarlet;

#[inline(never)]
pub fn force_link() {}
"#;
    write_if_changed(
        &project_dir.join(".scarlet/scarlet-modules/src/lib.rs"),
        modules_lib_rs,
    )?;

    let _ = write_if_changed(&project_dir.join(".gitignore"), ".scarlet\ntarget\n");

    eprintln!("cargo-scarlet: created project '{name}'");
    eprintln!("cargo-scarlet: REQUIRED: update .cargo/config.toml with runner");
    eprintln!("cargo-scarlet: REQUIRED: add linker script to lds/");
    eprintln!("cargo-scarlet: REQUIRED: implement boot entry in src/main.rs (arch_start_kernel)");

    Ok(())
}

fn inject_ksym_section(
    project: &Path,
    config: &ScarletConfig,
    target: Option<&str>,
    release: bool,
) -> Result<(), String> {
    let resolved_target = match target {
        Some(t) => t.to_string(),
        None => config.board.target_json.clone(),
    };
    let target_path = if Path::new(&resolved_target).is_absolute() {
        PathBuf::from(&resolved_target)
    } else {
        project.join(&resolved_target)
    };
    let target_triple = target_path
        .file_stem()
        .ok_or("target path has no file stem")?
        .to_string_lossy()
        .to_string();

    let profile = if release { "release" } else { "debug" };
    let binary_path = project
        .join("target")
        .join(&target_triple)
        .join(profile)
        .join("scarlet");

    if !binary_path.exists() {
        eprintln!(
            "cargo-scarlet: ksym: binary not found at {}, skipping",
            binary_path.display()
        );
        return Ok(());
    }

    let (nm_cmd, objcopy_cmd) = cross_tools_for_target(&target_triple);

    let nm_output = Command::new(&nm_cmd)
        .args([
            "--defined-only",
            "--extern-only",
            "-g",
            "--no-sort",
            binary_path.to_str().unwrap_or(""),
        ])
        .output()
        .map_err(|e| format!("failed to run nm: {e}"))?;

    if !nm_output.status.success() {
        eprintln!("cargo-scarlet: ksym: nm failed, skipping section injection");
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&nm_output.stdout);
    let mut symbols: Vec<(u64, String)> = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let addr_str = parts[0];
        let name = parts[2];

        if name.is_empty() {
            continue;
        }

        let skip = match name {
            "_GLOBAL_OFFSET_TABLE_" | "_DYNAMIC" => true,
            n if n.starts_with("__") && n.ends_with("_START") => true,
            n if n.starts_with("__") && n.ends_with("_END") => true,
            _ => false,
        };

        if skip {
            continue;
        }

        let addr = u64::from_str_radix(addr_str, 16).unwrap_or(0);
        symbols.push((addr, name.to_string()));
    }

    let count = symbols.len() as u64;
    let mut blob = Vec::new();
    blob.extend_from_slice(&count.to_le_bytes());

    for (addr, name) in &symbols {
        blob.extend_from_slice(&addr.to_le_bytes());
        let name_len = name.len() as u64;
        blob.extend_from_slice(&name_len.to_le_bytes());
        blob.extend_from_slice(name.as_bytes());
    }

    let tmp_dir = std::env::temp_dir().join("scarlet-ksym");
    fs::create_dir_all(&tmp_dir).map_err(|e| format!("failed to create temp dir: {e}"))?;
    let blob_path = tmp_dir.join("ksym_blob.bin");
    fs::write(&blob_path, &blob).map_err(|e| format!("failed to write ksym blob: {e}"))?;

    let update_status = Command::new(&objcopy_cmd)
        .args([
            "--update-section",
            &format!(".scarlet_ksyms={}", blob_path.display()),
            binary_path.to_str().unwrap_or(""),
        ])
        .status()
        .map_err(|e| format!("failed to run objcopy: {e}"))?;

    if !update_status.success() {
        return Err("objcopy failed to update .scarlet_ksyms section".to_string());
    }

    eprintln!(
        "cargo-scarlet: ksym: injected {} symbols into .scarlet_ksyms",
        count
    );

    let _ = fs::remove_file(&blob_path);
    Ok(())
}

fn cross_tools_for_target(target_triple: &str) -> (String, String) {
    let candidates: &[(&str, &[&str])] = &[
        (
            "riscv64",
            &[
                "riscv64-unknown-linux-gnu",
                "riscv64-linux-gnu",
                "riscv64-unknown-elf",
            ],
        ),
        (
            "aarch64",
            &[
                "aarch64-unknown-linux-gnu",
                "aarch64-linux-gnu",
                "aarch64-none-elf",
            ],
        ),
        (
            "x86_64",
            &["x86_64-unknown-linux-gnu", "x86_64-linux-gnu"],
        ),
    ];

    let prefixes = candidates
        .iter()
        .find(|(arch, _)| target_triple.starts_with(arch))
        .map(|(_, prefixes)| *prefixes)
        .unwrap_or(&[]);

    for prefix in prefixes {
        let nm = format!("{prefix}-nm");
        let objcopy = format!("{prefix}-objcopy");
        if which(&nm) && which(&objcopy) {
            return (nm, objcopy);
        }
    }

    if which("llvm-nm") && which("llvm-objcopy") {
        return ("llvm-nm".to_string(), "llvm-objcopy".to_string());
    }

    ("nm".to_string(), "objcopy".to_string())
}

fn which(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
