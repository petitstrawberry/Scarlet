use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

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
        project: Option<PathBuf>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        kernel_elf: Option<PathBuf>,
        #[arg(long)]
        no_build: bool,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long)]
        machine: Option<String>,
        #[arg(long)]
        distro: Option<String>,
        #[arg(long)]
        boot: Option<String>,
        #[arg(long = "image")]
        image_name: Option<String>,
    },
    Plan {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long)]
        machine: String,
        #[arg(long)]
        distro: String,
        #[arg(long)]
        boot: Option<String>,
        #[arg(long)]
        image: String,
        #[arg(long, default_value = "debug")]
        profile: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Lock {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ImageInputConfig {
    source: String,
    destination: String,
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    skip_suffixes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkspaceManifest {
    schema_version: u32,
    workspace: WorkspaceMetadata,
    #[serde(default)]
    layers: Vec<LayerSourceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkspaceMetadata {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LayerSourceConfig {
    name: String,
    path: Option<String>,
    git: Option<String>,
    rev: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    #[serde(default)]
    priority: i32,
}

#[derive(Debug, Deserialize)]
struct ScarletLocalManifest {
    #[serde(default)]
    overrides: LocalOverrides,
}

#[derive(Debug, Default, Deserialize)]
struct LocalOverrides {
    #[serde(default)]
    layers: Vec<LayerOverrideConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct LayerOverrideConfig {
    name: String,
    path: Option<String>,
    git: Option<String>,
    rev: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    priority: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct LayerManifest {
    schema_version: u32,
    layer: LayerMetadata,
    #[serde(default)]
    paths: LayerPaths,
}

#[derive(Debug, Deserialize)]
struct LayerMetadata {
    name: String,
    summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LayerPaths {
    #[serde(default = "default_machines_path")]
    machines: String,
    #[serde(default = "default_boot_targets_path")]
    boot_targets: String,
    #[serde(default = "default_distros_path")]
    distros: String,
    #[serde(default = "default_images_path")]
    images: String,
    #[serde(default = "default_recipes_path")]
    recipes: String,
    #[serde(default = "default_packagegroups_path")]
    packagegroups: String,
}

impl Default for LayerPaths {
    fn default() -> Self {
        Self {
            machines: default_machines_path(),
            boot_targets: default_boot_targets_path(),
            distros: default_distros_path(),
            images: default_images_path(),
            recipes: default_recipes_path(),
            packagegroups: default_packagegroups_path(),
        }
    }
}

#[derive(Debug)]
struct ResolvedLayer {
    source: LayerSourceConfig,
    manifest: LayerManifest,
    root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct MachineDocument {
    schema_version: u32,
    machine: MachineMetadata,
    build_adapter: Option<BuildAdapterConfig>,
}

#[derive(Debug, Deserialize)]
struct MachineMetadata {
    name: String,
    arch: String,
    target_triple: String,
    target_json: String,
    #[serde(default)]
    features: BTreeMap<String, bool>,
}

#[derive(Debug, Deserialize)]
struct BuildAdapterConfig {
    project: String,
}

#[derive(Debug, Deserialize)]
struct BootTargetDocument {
    schema_version: u32,
    boot_target: BootTargetConfig,
}

#[derive(Debug, Deserialize)]
struct BootTargetConfig {
    name: String,
    kind: String,
    arch: String,
    output: String,
    cmdline: Option<String>,
    image_slack_mb: Option<u32>,
    limine_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DistroDocument {
    schema_version: u32,
    distro: DistroMetadata,
    #[serde(default)]
    providers: BTreeMap<String, String>,
    #[serde(default)]
    features: BTreeMap<String, bool>,
}

#[derive(Debug, Deserialize)]
struct DistroMetadata {
    name: String,
    vendor: Option<String>,
    version: Option<String>,
    system_prefix: String,
    default_shell: Option<String>,
    default_boot_target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImageDocument {
    schema_version: u32,
    image: ImageRecipeMetadata,
    initramfs: Option<InitramfsConfig>,
    rootfs: Option<RootfsConfig>,
    #[serde(default)]
    files: Vec<ImageRecipeFile>,
}

#[derive(Debug, Deserialize)]
struct ImageRecipeMetadata {
    name: String,
    description: Option<String>,
    #[serde(default)]
    compatible_machines: Vec<String>,
    #[serde(default)]
    packagegroups: Vec<String>,
    #[serde(default)]
    packages: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ImageRecipeFile {
    source: String,
    destination: String,
}

#[derive(Debug, Deserialize)]
struct InitramfsConfig {
    format: String,
    output: String,
}

#[derive(Debug, Deserialize)]
struct RootfsConfig {
    format: String,
    output: String,
    source: String,
    user_bins: String,
    modules: String,
    stage: String,
    prebuilt: String,
}

#[derive(Debug, Deserialize)]
struct PackageGroupDocument {
    schema_version: u32,
    packagegroup: PackageGroupMetadata,
}

#[derive(Debug, Deserialize)]
struct PackageGroupMetadata {
    name: String,
    #[serde(default)]
    packages: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RecipeDocument {
    schema_version: u32,
    recipe: RecipeMetadata,
    #[serde(rename = "package", default)]
    packages: Vec<RecipePackage>,
}

#[derive(Debug, Deserialize)]
struct RecipeMetadata {
    name: String,
    source: Option<SourceConfig>,
}

#[derive(Debug, Deserialize)]
struct RecipePackage {
    name: String,
    #[serde(default)]
    files: Vec<RecipePackageFile>,
    #[serde(default)]
    bins: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RecipePackageFile {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
struct SourceConfig {
    path: Option<String>,
    git: Option<String>,
    rev: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkspaceLock {
    schema_version: u32,
    generated_by: String,
    workspace: LockWorkspace,
    layers: Vec<LockLayer>,
}

#[derive(Debug, Serialize)]
struct LockWorkspace {
    name: String,
}

#[derive(Debug, Serialize)]
struct LockLayer {
    name: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
    priority: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dirty: Option<bool>,
    local_override: bool,
}

#[derive(Debug)]
struct ResolvedMetadata<T> {
    document: T,
    path: PathBuf,
    layer_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeneratedImagePlan {
    schema_version: u32,
    generated_by: String,
    selection: PlanSelection,
    paths: PlanPaths,
    machine: PlanMachine,
    boot_target: PlanBootTarget,
    distro: PlanDistro,
    image: PlanImage,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    resolved_packages: Vec<PlanPackage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<ImageRecipeFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    steps: Vec<ImageStepConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlanSelection {
    workspace: String,
    machine: String,
    boot: String,
    distro: String,
    image: String,
    profile: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlanPaths {
    workspace_root: String,
    machine_metadata: String,
    boot_target_metadata: String,
    distro_metadata: String,
    image_metadata: String,
    machine_layer: String,
    boot_target_layer: String,
    distro_layer: String,
    image_layer: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlanMachine {
    arch: String,
    target_triple: String,
    target_json: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    features: BTreeMap<String, bool>,
    build_adapter_project: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlanBootTarget {
    kind: String,
    arch: String,
    output: String,
    cmdline: Option<String>,
    image_slack_mb: Option<u32>,
    limine_version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlanDistro {
    vendor: Option<String>,
    version: Option<String>,
    system_prefix: String,
    default_shell: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    providers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    features: BTreeMap<String, bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlanImage {
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    packagegroups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    packages: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlanPackage {
    name: String,
    recipe: String,
    layer: String,
    source: Option<PlanSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<ImageRecipeFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    bins: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlanSource {
    kind: String,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_machines_path() -> String {
    "machines".to_string()
}

fn default_boot_targets_path() -> String {
    "boot_targets".to_string()
}

fn default_distros_path() -> String {
    "distros".to_string()
}

fn default_images_path() -> String {
    "images".to_string()
}

fn default_recipes_path() -> String {
    "recipes".to_string()
}

fn default_packagegroups_path() -> String {
    "packagegroups".to_string()
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
            workspace,
            machine,
            distro,
            boot,
            image_name,
        } => match project {
            Some(project) => build_project_image(&project, target, release, kernel_elf, no_build),
            None => build_workspace_image(
                &workspace,
                machine.as_deref(),
                distro.as_deref(),
                boot.as_deref(),
                image_name.as_deref(),
                target,
                release,
                kernel_elf,
                no_build,
            ),
        },
        Commands::Plan {
            workspace,
            machine,
            distro,
            boot,
            image,
            profile,
            output,
        } => generate_image_plan(
            &workspace,
            &machine,
            &distro,
            boot.as_deref(),
            &image,
            &profile,
            output.as_deref(),
        )
        .map(|_| ()),
        Commands::Lock { workspace, output } => generate_lock(&workspace, output.as_deref()),
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

fn generate_image_plan(
    workspace: &Path,
    machine_name: &str,
    distro_name: &str,
    boot_name: Option<&str>,
    image_name: &str,
    profile: &str,
    output: Option<&Path>,
) -> Result<PathBuf, String> {
    let workspace_root = normalize_project_path(workspace)?;
    let (manifest, _) = load_workspace_manifest(&workspace_root)?;
    let layers = resolve_layers(&workspace_root, &manifest)?;
    let machine = resolve_metadata(&layers, "machine", machine_name, |layer| {
        layer
            .root
            .join(&layer.manifest.paths.machines)
            .join(format!("{machine_name}.toml"))
    })?;
    let distro = resolve_metadata(&layers, "distro", distro_name, |layer| {
        layer
            .root
            .join(&layer.manifest.paths.distros)
            .join(format!("{distro_name}.toml"))
    })?;
    validate_distro_document(&distro.document, &distro.path, distro_name)?;
    let boot_name = boot_name
        .or(distro.document.distro.default_boot_target.as_deref())
        .ok_or("--boot is required because the selected distro has no default_boot_target")?;
    let boot_target = resolve_metadata(&layers, "boot target", boot_name, |layer| {
        layer
            .root
            .join(&layer.manifest.paths.boot_targets)
            .join(format!("{boot_name}.toml"))
    })?;
    let image = resolve_metadata(&layers, "image", image_name, |layer| {
        layer
            .root
            .join(&layer.manifest.paths.images)
            .join(format!("{image_name}.toml"))
    })?;

    validate_machine_document(&machine.document, &machine.path, machine_name)?;
    validate_boot_target_document(&boot_target.document, &boot_target.path, boot_name)?;
    if boot_target.document.boot_target.arch != machine.document.machine.arch {
        return Err(format!(
            "{}: boot_target.arch `{}` is not compatible with machine `{}` arch `{}`",
            boot_target.path.display(),
            boot_target.document.boot_target.arch,
            machine.document.machine.name,
            machine.document.machine.arch
        ));
    }
    validate_image_document(&image.document, &image.path, image_name, machine_name)?;
    let resolved_packages = resolve_image_packages(&layers, &image.document)?;

    let plan = render_plan(
        &workspace_root,
        &manifest,
        &machine,
        &boot_target,
        &distro,
        &image,
        resolved_packages,
        profile,
    )?;

    let output = match output {
        Some(path) => absolutize_from_current_dir(path)?,
        None => workspace_root
            .join(".scarlet/plans")
            .join(machine_name)
            .join(boot_name)
            .join(distro_name)
            .join(image_name)
            .join(profile)
            .join("image-plan.toml"),
    };

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let rendered = toml::to_string_pretty(&plan)
        .map_err(|error| format!("failed to render image plan: {error}"))?;
    write_if_changed(&output, &rendered)?;
    eprintln!("cargo-scarlet: wrote image plan {}", output.display());
    Ok(output)
}

fn generate_lock(workspace: &Path, output: Option<&Path>) -> Result<(), String> {
    let workspace_root = normalize_project_path(workspace)?;
    let (manifest, overridden_layers) = load_workspace_manifest(&workspace_root)?;
    let layers = resolve_layers(&workspace_root, &manifest)?;

    let lock = WorkspaceLock {
        schema_version: 1,
        generated_by: "cargo-scarlet lock".to_string(),
        workspace: LockWorkspace {
            name: manifest.workspace.name,
        },
        layers: layers
            .into_iter()
            .map(|layer| lock_layer(layer, &overridden_layers))
            .collect::<Result<Vec<_>, _>>()?,
    };

    let output = match output {
        Some(path) => absolutize_from_current_dir(path)?,
        None => workspace_root.join("scarlet.lock"),
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let rendered = toml::to_string_pretty(&lock)
        .map_err(|error| format!("failed to render lock file: {error}"))?;
    write_if_changed(&output, &rendered)?;
    eprintln!("cargo-scarlet: wrote lock file {}", output.display());
    Ok(())
}

fn lock_layer(layer: ResolvedLayer, overridden_layers: &[String]) -> Result<LockLayer, String> {
    let resolved_rev = git_text(&layer.root, &["rev-parse", "HEAD"]).ok();
    let dirty = git_text(&layer.root, &["status", "--porcelain"])
        .ok()
        .map(|status| !status.trim().is_empty());
    let source = if layer.source.git.is_some() {
        "git"
    } else {
        "path"
    };

    Ok(LockLayer {
        name: layer.source.name.clone(),
        source: source.to_string(),
        path: layer.source.path.clone(),
        git: layer.source.git.clone(),
        rev: layer.source.rev.clone(),
        branch: layer.source.branch.clone(),
        tag: layer.source.tag.clone(),
        priority: layer.source.priority,
        resolved_rev,
        dirty,
        local_override: overridden_layers
            .iter()
            .any(|name| name == &layer.source.name),
    })
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git in {}: {error}", repo.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git -C {} {} failed",
            repo.display(),
            args.join(" ")
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn read_toml_file<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn load_workspace_manifest(
    workspace_root: &Path,
) -> Result<(WorkspaceManifest, Vec<String>), String> {
    let manifest_path = workspace_root.join("scarlet.toml");
    let mut manifest: WorkspaceManifest = read_toml_file(&manifest_path)?;
    validate_workspace_manifest(&manifest, &manifest_path)?;

    let overridden_layers = apply_local_overrides(workspace_root, &mut manifest)?;
    validate_workspace_manifest(&manifest, &manifest_path)?;
    Ok((manifest, overridden_layers))
}

fn apply_local_overrides(
    workspace_root: &Path,
    manifest: &mut WorkspaceManifest,
) -> Result<Vec<String>, String> {
    let local_path = workspace_root.join("scarlet.local.toml");
    if !local_path.exists() {
        return Ok(Vec::new());
    }

    let local: ScarletLocalManifest = read_toml_file(&local_path)?;
    let mut overridden_layers = Vec::new();
    for layer_override in local.overrides.layers {
        if layer_override.name.trim().is_empty() {
            return Err(format!(
                "{}: overrides.layers.name must not be empty",
                local_path.display()
            ));
        }

        let layer = match manifest
            .layers
            .iter_mut()
            .find(|layer| layer.name == layer_override.name)
        {
            Some(layer) => layer,
            None => {
                manifest.layers.push(LayerSourceConfig {
                    name: layer_override.name.clone(),
                    path: None,
                    git: None,
                    rev: None,
                    branch: None,
                    tag: None,
                    priority: layer_override.priority.unwrap_or_default(),
                });
                manifest
                    .layers
                    .last_mut()
                    .expect("newly pushed local override layer must exist")
            }
        };

        if let Some(path) = layer_override.path {
            layer.path = Some(path);
        }
        if let Some(git) = layer_override.git {
            layer.git = Some(git);
        }
        if let Some(rev) = layer_override.rev {
            layer.rev = Some(rev);
        }
        if let Some(branch) = layer_override.branch {
            layer.branch = Some(branch);
        }
        if let Some(tag) = layer_override.tag {
            layer.tag = Some(tag);
        }
        if let Some(priority) = layer_override.priority {
            layer.priority = priority;
        }

        push_unique(&mut overridden_layers, layer.name.clone());
    }

    Ok(overridden_layers)
}

fn validate_workspace_manifest(
    manifest: &WorkspaceManifest,
    manifest_path: &Path,
) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            manifest_path.display(),
            manifest.schema_version
        ));
    }

    if manifest.workspace.name.trim().is_empty() {
        return Err(format!(
            "{}: workspace.name must not be empty",
            manifest_path.display()
        ));
    }

    if manifest.layers.is_empty() {
        return Err(format!(
            "{}: at least one [[layers]] entry is required",
            manifest_path.display()
        ));
    }

    let mut layer_names = Vec::new();
    for layer in &manifest.layers {
        if layer.name.trim().is_empty() {
            return Err(format!(
                "{}: layer names must not be empty",
                manifest_path.display()
            ));
        }
        if layer_names.iter().any(|existing| existing == &layer.name) {
            return Err(format!(
                "{}: duplicate layer `{}`",
                manifest_path.display(),
                layer.name
            ));
        }
        layer_names.push(layer.name.clone());

        if layer.path.is_none() && layer.git.is_none() {
            return Err(format!(
                "{}: layer `{}` must specify path or git",
                manifest_path.display(),
                layer.name
            ));
        }

        if layer.git.is_some()
            && layer.rev.is_none()
            && layer.branch.is_none()
            && layer.tag.is_none()
        {
            return Err(format!(
                "{}: git layer `{}` must specify rev, branch, or tag",
                manifest_path.display(),
                layer.name
            ));
        }
    }

    Ok(())
}

fn resolve_layers(
    workspace_root: &Path,
    manifest: &WorkspaceManifest,
) -> Result<Vec<ResolvedLayer>, String> {
    let mut layers = Vec::new();

    for source in &manifest.layers {
        let Some(path) = &source.path else {
            return Err(format!(
                "layer `{}` uses git without a local path; fetch/update is not implemented yet",
                source.name
            ));
        };
        let root = absolutize_from_base(workspace_root, Path::new(path))?;
        let manifest_path = root.join("layer.toml");
        let layer_manifest: LayerManifest = read_toml_file(&manifest_path)?;
        validate_layer_manifest(&layer_manifest, &manifest_path, &source.name)?;
        validate_layer_paths(&root, &layer_manifest, &manifest_path)?;
        layers.push(ResolvedLayer {
            source: LayerSourceConfig {
                name: source.name.clone(),
                path: source.path.clone(),
                git: source.git.clone(),
                rev: source.rev.clone(),
                branch: source.branch.clone(),
                tag: source.tag.clone(),
                priority: source.priority,
            },
            manifest: layer_manifest,
            root,
        });
    }

    layers.sort_by(|left, right| {
        right
            .source
            .priority
            .cmp(&left.source.priority)
            .then_with(|| left.source.name.cmp(&right.source.name))
    });
    Ok(layers)
}

fn validate_layer_manifest(
    manifest: &LayerManifest,
    manifest_path: &Path,
    expected_name: &str,
) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            manifest_path.display(),
            manifest.schema_version
        ));
    }

    if manifest.layer.name != expected_name {
        return Err(format!(
            "{}: layer.name `{}` does not match scarlet.toml entry `{expected_name}`",
            manifest_path.display(),
            manifest.layer.name
        ));
    }

    if manifest.layer.summary.as_deref().is_some_and(str::is_empty) {
        return Err(format!(
            "{}: layer.summary must not be empty when present",
            manifest_path.display()
        ));
    }

    Ok(())
}

fn validate_layer_paths(
    root: &Path,
    manifest: &LayerManifest,
    manifest_path: &Path,
) -> Result<(), String> {
    for (field, value) in [
        ("paths.machines", &manifest.paths.machines),
        ("paths.boot_targets", &manifest.paths.boot_targets),
        ("paths.distros", &manifest.paths.distros),
        ("paths.images", &manifest.paths.images),
        ("paths.recipes", &manifest.paths.recipes),
        ("paths.packagegroups", &manifest.paths.packagegroups),
    ] {
        if value.trim().is_empty() {
            return Err(format!(
                "{}: {field} must not be empty",
                manifest_path.display()
            ));
        }

        let path = Path::new(value);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(format!(
                "{}: {field} must be a relative path inside the layer",
                manifest_path.display()
            ));
        }

        let full_path = root.join(path);
        if !full_path.exists() {
            return Err(format!(
                "{}: {field} points to missing path {}",
                manifest_path.display(),
                full_path.display()
            ));
        }
    }

    Ok(())
}

fn resolve_metadata<T, F>(
    layers: &[ResolvedLayer],
    kind: &str,
    name: &str,
    path_for_layer: F,
) -> Result<ResolvedMetadata<T>, String>
where
    T: DeserializeOwned,
    F: Fn(&ResolvedLayer) -> PathBuf,
{
    for layer in layers {
        let path = path_for_layer(layer);
        if path.exists() {
            let document = read_toml_file(&path)?;
            return Ok(ResolvedMetadata {
                document,
                path,
                layer_name: layer.manifest.layer.name.clone(),
            });
        }
    }

    Err(format!(
        "{kind} `{name}` was not found in any configured layer"
    ))
}

fn validate_machine_document(
    document: &MachineDocument,
    path: &Path,
    expected_name: &str,
) -> Result<(), String> {
    if document.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            path.display(),
            document.schema_version
        ));
    }
    if document.machine.name != expected_name {
        return Err(format!(
            "{}: machine.name `{}` does not match requested machine `{expected_name}`",
            path.display(),
            document.machine.name
        ));
    }
    if document.machine.arch.trim().is_empty() {
        return Err(format!(
            "{}: machine.arch must not be empty",
            path.display()
        ));
    }
    if document.machine.target_triple.trim().is_empty() {
        return Err(format!(
            "{}: machine.target_triple must not be empty",
            path.display()
        ));
    }
    if document.machine.target_json.trim().is_empty() {
        return Err(format!(
            "{}: machine.target_json must not be empty",
            path.display()
        ));
    }
    Ok(())
}

fn validate_boot_target_document(
    document: &BootTargetDocument,
    path: &Path,
    expected_name: &str,
) -> Result<(), String> {
    if document.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            path.display(),
            document.schema_version
        ));
    }
    if document.boot_target.name != expected_name {
        return Err(format!(
            "{}: boot_target.name `{}` does not match requested boot target `{expected_name}`",
            path.display(),
            document.boot_target.name
        ));
    }
    if document.boot_target.kind.trim().is_empty() {
        return Err(format!(
            "{}: boot_target.kind must not be empty",
            path.display()
        ));
    }
    if document.boot_target.arch.trim().is_empty() {
        return Err(format!(
            "{}: boot_target.arch must not be empty",
            path.display()
        ));
    }
    if document.boot_target.output.trim().is_empty() {
        return Err(format!(
            "{}: boot_target.output must not be empty",
            path.display()
        ));
    }
    Ok(())
}

fn validate_distro_document(
    document: &DistroDocument,
    path: &Path,
    expected_name: &str,
) -> Result<(), String> {
    if document.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            path.display(),
            document.schema_version
        ));
    }
    if document.distro.name != expected_name {
        return Err(format!(
            "{}: distro.name `{}` does not match requested distro `{expected_name}`",
            path.display(),
            document.distro.name
        ));
    }
    if document.distro.system_prefix.trim().is_empty() {
        return Err(format!(
            "{}: distro.system_prefix must not be empty",
            path.display()
        ));
    }
    Ok(())
}

fn validate_image_document(
    document: &ImageDocument,
    path: &Path,
    expected_name: &str,
    machine_name: &str,
) -> Result<(), String> {
    if document.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            path.display(),
            document.schema_version
        ));
    }
    if document.image.name != expected_name {
        return Err(format!(
            "{}: image.name `{}` does not match requested image `{expected_name}`",
            path.display(),
            document.image.name
        ));
    }
    if !document.image.compatible_machines.is_empty()
        && !document
            .image
            .compatible_machines
            .iter()
            .any(|candidate| candidate == machine_name)
    {
        return Err(format!(
            "{}: image `{}` is not compatible with machine `{machine_name}`",
            path.display(),
            document.image.name
        ));
    }
    for (index, file) in document.files.iter().enumerate() {
        if file.source.trim().is_empty() {
            return Err(format!(
                "{}: files[{index}].source is empty",
                path.display()
            ));
        }
        if file.destination.trim().is_empty() {
            return Err(format!(
                "{}: files[{index}].destination is empty",
                path.display()
            ));
        }
    }
    if let Some(initramfs) = &document.initramfs {
        if initramfs.format != "newc" {
            return Err(format!(
                "{}: unsupported initramfs.format `{}`",
                path.display(),
                initramfs.format
            ));
        }
        if initramfs.output.trim().is_empty() {
            return Err(format!(
                "{}: initramfs.output must not be empty",
                path.display()
            ));
        }
    }
    if let Some(rootfs) = &document.rootfs {
        if rootfs.format != "ext2" {
            return Err(format!(
                "{}: unsupported rootfs.format `{}`",
                path.display(),
                rootfs.format
            ));
        }
        for (field, value) in [
            ("rootfs.output", &rootfs.output),
            ("rootfs.source", &rootfs.source),
            ("rootfs.user_bins", &rootfs.user_bins),
            ("rootfs.modules", &rootfs.modules),
            ("rootfs.stage", &rootfs.stage),
            ("rootfs.prebuilt", &rootfs.prebuilt),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{}: {field} must not be empty", path.display()));
            }
        }
    }
    Ok(())
}

fn resolve_image_packages(
    layers: &[ResolvedLayer],
    image: &ImageDocument,
) -> Result<Vec<PlanPackage>, String> {
    let mut package_names = Vec::new();
    for packagegroup in &image.image.packagegroups {
        let group = resolve_packagegroup(layers, packagegroup)?;
        for package in &group.document.packagegroup.packages {
            push_unique(&mut package_names, package.clone());
        }
    }
    for package in &image.image.packages {
        push_unique(&mut package_names, package.clone());
    }

    let mut resolved = Vec::new();
    for package in package_names {
        resolved.push(resolve_package_provider(layers, &package)?);
    }
    Ok(resolved)
}

fn resolve_packagegroup(
    layers: &[ResolvedLayer],
    name: &str,
) -> Result<ResolvedMetadata<PackageGroupDocument>, String> {
    let group: ResolvedMetadata<PackageGroupDocument> =
        resolve_metadata(layers, "packagegroup", name, |layer| {
            layer
                .root
                .join(&layer.manifest.paths.packagegroups)
                .join(format!("{name}.toml"))
        })?;

    if group.document.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            group.path.display(),
            group.document.schema_version
        ));
    }
    if group.document.packagegroup.name != name {
        return Err(format!(
            "{}: packagegroup.name `{}` does not match requested packagegroup `{name}`",
            group.path.display(),
            group.document.packagegroup.name
        ));
    }

    Ok(group)
}

fn resolve_package_provider(
    layers: &[ResolvedLayer],
    package: &str,
) -> Result<PlanPackage, String> {
    for layer in layers {
        let recipe_dir = layer.root.join(&layer.manifest.paths.recipes);
        for entry in sorted_dir_entries(&recipe_dir)? {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "toml") {
                continue;
            }

            let recipe: RecipeDocument = read_toml_file(&path)?;
            validate_recipe_document(&recipe, &path)?;
            if let Some(candidate) = recipe
                .packages
                .iter()
                .find(|candidate| candidate.name == package)
            {
                let recipe_dir = path
                    .parent()
                    .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
                let source_root = recipe
                    .recipe
                    .source
                    .as_ref()
                    .and_then(|source| source.path.as_ref())
                    .map(|source_path| {
                        render_workspace_template(source_path, Path::new("."), recipe_dir)
                    });
                return Ok(PlanPackage {
                    name: package.to_string(),
                    recipe: recipe.recipe.name,
                    layer: layer.manifest.layer.name.clone(),
                    source: recipe.recipe.source.as_ref().map(plan_source),
                    files: render_package_files(source_root.as_deref(), candidate),
                    bins: candidate.bins.clone(),
                });
            }
        }
    }

    Err(format!(
        "package `{package}` is selected by the image but no recipe provides it"
    ))
}

fn validate_recipe_document(document: &RecipeDocument, path: &Path) -> Result<(), String> {
    if document.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            path.display(),
            document.schema_version
        ));
    }
    if document.recipe.name.trim().is_empty() {
        return Err(format!("{}: recipe.name must not be empty", path.display()));
    }
    if let Some(source) = &document.recipe.source {
        validate_source_config(source, path, "recipe.source")?;
    }
    if document.packages.is_empty() {
        return Err(format!(
            "{}: at least one [[package]] is required",
            path.display()
        ));
    }
    for (index, package) in document.packages.iter().enumerate() {
        if package.name.trim().is_empty() {
            return Err(format!(
                "{}: package[{index}].name is empty",
                path.display()
            ));
        }
        for (file_index, file) in package.files.iter().enumerate() {
            if file.from.trim().is_empty() {
                return Err(format!(
                    "{}: package[{index}].files[{file_index}].from is empty",
                    path.display()
                ));
            }
            if file.to.trim().is_empty() {
                return Err(format!(
                    "{}: package[{index}].files[{file_index}].to is empty",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn plan_source(source: &SourceConfig) -> PlanSource {
    if let Some(path) = &source.path {
        PlanSource {
            kind: "path".to_string(),
            value: path.clone(),
            rev: source.rev.clone(),
            branch: source.branch.clone(),
            tag: source.tag.clone(),
        }
    } else {
        PlanSource {
            kind: "git".to_string(),
            value: source.git.clone().unwrap_or_default(),
            rev: source.rev.clone(),
            branch: source.branch.clone(),
            tag: source.tag.clone(),
        }
    }
}

fn render_package_files(
    source_root: Option<&str>,
    package: &RecipePackage,
) -> Vec<ImageRecipeFile> {
    package
        .files
        .iter()
        .map(|file| {
            let source = match source_root {
                Some(source_root) => Path::new(source_root)
                    .join(&file.from)
                    .display()
                    .to_string(),
                None => file.from.clone(),
            };
            ImageRecipeFile {
                source,
                destination: file.to.clone(),
            }
        })
        .collect()
}

fn validate_source_config(source: &SourceConfig, path: &Path, field: &str) -> Result<(), String> {
    if source.path.is_none() && source.git.is_none() {
        return Err(format!(
            "{}: {field} must specify path or git",
            path.display()
        ));
    }

    if let Some(source_path) = &source.path {
        if source_path.trim().is_empty() {
            return Err(format!(
                "{}: {field}.path must not be empty",
                path.display()
            ));
        }
        let metadata_dir = path
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
        let full_path = render_workspace_template(source_path, Path::new("."), metadata_dir);
        let full_path = Path::new(&full_path);
        if !full_path.exists() {
            return Err(format!(
                "{}: {field}.path points to missing path {}",
                path.display(),
                full_path.display()
            ));
        }
    }

    if source.git.is_some()
        && source.rev.is_none()
        && source.branch.is_none()
        && source.tag.is_none()
    {
        return Err(format!(
            "{}: {field}.git must specify rev, branch, or tag",
            path.display()
        ));
    }

    Ok(())
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn render_plan(
    workspace_root: &Path,
    manifest: &WorkspaceManifest,
    machine: &ResolvedMetadata<MachineDocument>,
    boot_target: &ResolvedMetadata<BootTargetDocument>,
    distro: &ResolvedMetadata<DistroDocument>,
    image: &ResolvedMetadata<ImageDocument>,
    resolved_packages: Vec<PlanPackage>,
    profile: &str,
) -> Result<GeneratedImagePlan, String> {
    let machine_dir = machine
        .path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", machine.path.display()))?;
    let target_json = render_workspace_template(
        &machine.document.machine.target_json,
        workspace_root,
        machine_dir,
    );
    let build_adapter_project =
        machine.document.build_adapter.as_ref().map(|adapter| {
            render_workspace_template(&adapter.project, workspace_root, machine_dir)
        });

    let boot_target_dir = boot_target
        .path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", boot_target.path.display()))?;
    let image_dir = image
        .path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", image.path.display()))?;
    let mut files = image
        .document
        .files
        .iter()
        .map(|file| ImageRecipeFile {
            source: render_workspace_template(&file.source, workspace_root, image_dir),
            destination: file.destination.clone(),
        })
        .collect::<Vec<_>>();
    for package in &resolved_packages {
        for file in &package.files {
            push_plan_file(&mut files, file.clone())?;
        }
    }
    let steps = render_plan_steps(workspace_root, machine, boot_target, image, &files)?;

    Ok(GeneratedImagePlan {
        schema_version: 1,
        generated_by: "cargo-scarlet plan".to_string(),
        selection: PlanSelection {
            workspace: manifest.workspace.name.clone(),
            machine: machine.document.machine.name.clone(),
            boot: boot_target.document.boot_target.name.clone(),
            distro: distro.document.distro.name.clone(),
            image: image.document.image.name.clone(),
            profile: profile.to_string(),
        },
        paths: PlanPaths {
            workspace_root: workspace_root.display().to_string(),
            machine_metadata: machine.path.display().to_string(),
            boot_target_metadata: boot_target.path.display().to_string(),
            distro_metadata: distro.path.display().to_string(),
            image_metadata: image.path.display().to_string(),
            machine_layer: machine.layer_name.clone(),
            boot_target_layer: boot_target.layer_name.clone(),
            distro_layer: distro.layer_name.clone(),
            image_layer: image.layer_name.clone(),
        },
        machine: PlanMachine {
            arch: machine.document.machine.arch.clone(),
            target_triple: machine.document.machine.target_triple.clone(),
            target_json,
            features: machine.document.machine.features.clone(),
            build_adapter_project,
        },
        boot_target: PlanBootTarget {
            kind: boot_target.document.boot_target.kind.clone(),
            arch: boot_target.document.boot_target.arch.clone(),
            output: render_workspace_template(
                &boot_target.document.boot_target.output,
                workspace_root,
                boot_target_dir,
            ),
            cmdline: boot_target.document.boot_target.cmdline.clone(),
            image_slack_mb: boot_target.document.boot_target.image_slack_mb,
            limine_version: boot_target.document.boot_target.limine_version.clone(),
        },
        distro: PlanDistro {
            vendor: distro.document.distro.vendor.clone(),
            version: distro.document.distro.version.clone(),
            system_prefix: distro.document.distro.system_prefix.clone(),
            default_shell: distro.document.distro.default_shell.clone(),
            providers: distro.document.providers.clone(),
            features: distro.document.features.clone(),
        },
        image: PlanImage {
            description: image.document.image.description.clone(),
            packagegroups: image.document.image.packagegroups.clone(),
            packages: image.document.image.packages.clone(),
        },
        resolved_packages,
        files,
        steps,
    })
}

fn push_plan_file(files: &mut Vec<ImageRecipeFile>, file: ImageRecipeFile) -> Result<(), String> {
    if let Some(existing) = files
        .iter()
        .find(|existing| existing.destination == file.destination)
    {
        if existing.source == file.source {
            return Ok(());
        }
        return Err(format!(
            "multiple selected files install to {}: {} and {}",
            file.destination, existing.source, file.source
        ));
    }
    files.push(file);
    Ok(())
}

fn render_plan_steps(
    workspace_root: &Path,
    machine: &ResolvedMetadata<MachineDocument>,
    boot_target: &ResolvedMetadata<BootTargetDocument>,
    image: &ResolvedMetadata<ImageDocument>,
    files: &[ImageRecipeFile],
) -> Result<Vec<ImageStepConfig>, String> {
    let mut steps = Vec::new();
    let image_dir = image
        .path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", image.path.display()))?;

    if let Some(initramfs) = &image.document.initramfs {
        steps.push(ImageStepConfig {
            name: Some("initramfs".to_string()),
            kind: Some("archive.newc".to_string()),
            command: None,
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            inputs: files
                .iter()
                .map(|file| ImageInputConfig {
                    source: file.source.clone(),
                    destination: file.destination.clone(),
                    optional: false,
                    skip_suffixes: Vec::new(),
                })
                .collect(),
            output: Some(render_workspace_template(
                &initramfs.output,
                workspace_root,
                image_dir,
            )),
        });
    }

    if let Some(rootfs) = &image.document.rootfs {
        let install_manifest_output = format!(
            "{{project}}/.scarlet/images/{}-install-manifest.tsv",
            image.document.image.name
        );
        steps.push(ImageStepConfig {
            name: Some("install-manifest".to_string()),
            kind: Some("file.manifest".to_string()),
            command: None,
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            inputs: files
                .iter()
                .filter(|file| file.destination != "/")
                .map(|file| ImageInputConfig {
                    source: file.source.clone(),
                    destination: file.destination.clone(),
                    optional: false,
                    skip_suffixes: Vec::new(),
                })
                .collect(),
            output: Some(install_manifest_output.clone()),
        });
        steps.push(ImageStepConfig {
            name: Some("rootfs".to_string()),
            kind: None,
            command: Some("{project}/tools/build_rootfs_ext2.sh".to_string()),
            args: vec![
                render_workspace_template(&rootfs.source, workspace_root, image_dir),
                render_workspace_template(&rootfs.user_bins, workspace_root, image_dir),
                render_workspace_template(&rootfs.modules, workspace_root, image_dir),
                render_workspace_template(&rootfs.stage, workspace_root, image_dir),
                render_workspace_template(&rootfs.output, workspace_root, image_dir),
                render_workspace_template(&rootfs.prebuilt, workspace_root, image_dir),
                install_manifest_output,
            ],
            cwd: None,
            env: BTreeMap::new(),
            inputs: Vec::new(),
            output: Some(render_workspace_template(
                &rootfs.output,
                workspace_root,
                image_dir,
            )),
        });
    }

    let boot_target_config = &boot_target.document.boot_target;
    match boot_target_config.kind.as_str() {
        "limine" | "limine-uefi" => {
            let initramfs_output = image
                .document
                .initramfs
                .as_ref()
                .ok_or("limine boot target requires image initramfs")?
                .output
                .clone();
            let boot_target_dir = boot_target
                .path
                .parent()
                .ok_or_else(|| format!("{} has no parent directory", boot_target.path.display()))?;
            let mut args = vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                "{repo}/cargo-scarlet-plugin-limine/Cargo.toml".to_string(),
                "--".to_string(),
                "--arch".to_string(),
                machine.document.machine.arch.clone(),
                "--kernel".to_string(),
                "{kernel_elf}".to_string(),
                "--initramfs".to_string(),
                render_workspace_template(&initramfs_output, workspace_root, image_dir),
                "--output".to_string(),
                render_workspace_template(
                    &boot_target_config.output,
                    workspace_root,
                    boot_target_dir,
                ),
            ];

            if let Some(cmdline) = &boot_target_config.cmdline {
                args.push("--cmdline".to_string());
                args.push(cmdline.clone());
            }
            if let Some(image_slack_mb) = boot_target_config.image_slack_mb {
                args.push("--image-slack-mb".to_string());
                args.push(image_slack_mb.to_string());
            }
            if let Some(limine_version) = &boot_target_config.limine_version {
                args.push("--limine-version".to_string());
                args.push(limine_version.clone());
            }

            steps.push(ImageStepConfig {
                name: Some("boot-image".to_string()),
                kind: None,
                command: Some("cargo".to_string()),
                args,
                cwd: Some("{repo}".to_string()),
                env: BTreeMap::new(),
                inputs: Vec::new(),
                output: Some(render_workspace_template(
                    &boot_target_config.output,
                    workspace_root,
                    boot_target_dir,
                )),
            });
        }
        other => {
            return Err(format!("unsupported boot_target.kind `{other}`"));
        }
    }

    Ok(steps)
}

fn render_workspace_template(value: &str, workspace_root: &Path, metadata_dir: &Path) -> String {
    let rendered = value.replace("{workspace}", &workspace_root.display().to_string());
    if rendered.starts_with('{') {
        return rendered;
    }
    let path = Path::new(&rendered);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        metadata_dir.join(path)
    };
    if rendered.contains('{') {
        return path.display().to_string();
    }
    fs::canonicalize(&path)
        .unwrap_or(path)
        .display()
        .to_string()
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

fn build_workspace_image(
    workspace: &Path,
    machine: Option<&str>,
    distro: Option<&str>,
    boot: Option<&str>,
    image: Option<&str>,
    target: Option<String>,
    release: bool,
    kernel_elf: Option<PathBuf>,
    no_build: bool,
) -> Result<(), String> {
    let machine = machine.ok_or("--machine is required when --project is not used")?;
    let distro = distro.ok_or("--distro is required when --project is not used")?;
    let image = image.ok_or("--image is required when --project is not used")?;
    let profile = if release { "release" } else { "debug" };
    let plan_path = generate_image_plan(workspace, machine, distro, boot, image, profile, None)?;
    let plan: GeneratedImagePlan = read_toml_file(&plan_path)?;

    if plan.schema_version != 1 {
        return Err(format!(
            "{} uses unsupported schema_version {} (expected 1)",
            plan_path.display(),
            plan.schema_version
        ));
    }

    let project = plan
        .machine
        .build_adapter_project
        .as_ref()
        .ok_or("selected machine has no build_adapter_project; native workspace builds are not implemented yet")?;
    let project = PathBuf::from(project);
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

    if plan.steps.is_empty() {
        return Err(format!("{} has no image steps", plan_path.display()));
    }

    let target_triple = target_triple_for_project(&project, &config, target.as_deref())?;
    for (index, step) in plan.steps.iter().enumerate() {
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
        "file.manifest" => write_install_manifest(
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

fn write_install_manifest(
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
        "file.manifest output",
        project,
        config,
        kernel_elf,
        profile,
        target_triple,
    )?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let mut rendered = String::new();
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
        if !source.exists() && !input.optional {
            return Err(format!("image input not found: {}", source.display()));
        }
        if !source.exists() {
            continue;
        }
        let destination = normalize_archive_path(&render_image_template(
            &input.destination,
            project,
            config,
            kernel_elf,
            profile,
            target_triple,
        ))?;
        let _ = writeln!(
            &mut rendered,
            "{}\t/{}",
            source.display(),
            destination.display()
        );
    }

    write_if_changed(&output, &rendered)?;
    eprintln!(
        "cargo-scarlet: image {step_name}: wrote install manifest {}",
        output.display()
    );
    Ok(())
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
        ("x86_64", &["x86_64-unknown-linux-gnu", "x86_64-linux-gnu"]),
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
