use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
        #[arg(long)]
        no_image: bool,
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
    Update {
        #[arg(long)]
        project: PathBuf,
    },
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum PackageSource {
    Path(String),
    Git {
        git: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
}

impl PackageSource {
    fn to_local_path(&self, base_dir: &Path) -> Option<PathBuf> {
        match self {
            PackageSource::Path(p) => Some(resolve_path(base_dir, p)),
            PackageSource::Git { .. } => None,
        }
    }

    fn is_git(&self) -> bool {
        matches!(self, PackageSource::Git { .. })
    }

    fn git_url(&self) -> Option<&str> {
        match self {
            PackageSource::Git { git, .. } => Some(git),
            _ => None,
        }
    }

    fn git_ref(&self) -> Option<String> {
        match self {
            PackageSource::Git {
                branch: Some(b), ..
            } => Some(format!("refs/heads/{b}")),
            PackageSource::Git { tag: Some(t), .. } => Some(format!("refs/tags/{t}")),
            PackageSource::Git { rev: Some(r), .. } => Some(r.clone()),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScarletManifest {
    schema_version: u32,
    #[allow(dead_code)]
    project: ManifestProject,
    kernel: ManifestKernel,
    #[serde(default)]
    modules: BTreeMap<String, ModuleConfig>,
    #[serde(default)]
    images: BTreeMap<String, ManifestImageSection>,
    #[serde(default)]
    runner: Option<ManifestRunner>,
}

#[derive(Debug, Deserialize)]
struct ManifestRunner {
    command: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ManifestProject {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ManifestKernel {
    package: String,
    source: PackageSource,
    target: String,
    target_json: String,
    #[serde(default)]
    features: BTreeMap<String, bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ManifestImageSection {
    format: Option<String>,
    output: Option<String>,
    #[serde(default)]
    cmdline: String,
    #[serde(default)]
    packages: Vec<ManifestPackage>,
    #[serde(default)]
    files: Vec<ManifestFileEntry>,
    #[serde(default)]
    deps: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ManifestPackage {
    kind: Option<String>,
    source: Option<PackageSource>,
    package: Option<String>,
    bin: Option<String>,
    from: Option<String>,
    to: String,
    output: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ManifestFileEntry {
    source: String,
    to: String,
    #[serde(default)]
    template: bool,
}

struct ResolvedSection {
    packages: Vec<ResolvedPackage>,
    files: Vec<ResolvedFile>,
}

struct ExpandedManifest {
    #[allow(dead_code)]
    project_dir: PathBuf,
    manifest: ScarletManifest,
    sections: BTreeMap<String, ResolvedSection>,
}

#[derive(Serialize)]
struct PluginRequest<'a> {
    project_dir: String,
    section_name: &'a str,
    format: &'a str,
    arch: String,
    kernel_elf: String,
    initramfs: Option<String>,
    output: String,
    section: &'a ManifestImageSection,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct ImageLock {
    #[serde(default)]
    sections: BTreeMap<String, SectionLock>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SectionLock {
    hash: String,
    #[serde(default)]
    files: Vec<FileLock>,
    #[serde(default)]
    packages: Vec<PackageLock>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FileLock {
    source: String,
    hash: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PackageLock {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    hash: String,
}

struct ResolvedPackage {
    kind: Option<String>,
    source: Option<PackageSource>,
    local_source: Option<PathBuf>,
    resolved_rev: Option<String>,
    package_name: Option<String>,
    bin: Option<String>,
    from: Option<PathBuf>,
    from_image: Option<String>,
    to: String,
    output: Option<PathBuf>,
}

fn load_manifest(project_dir: &Path) -> Result<ScarletManifest, String> {
    let manifest_path = project_dir.join("scarlet.toml");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("failed to read {}: {e}", manifest_path.display()))?;
    let mut root: toml::Value = toml::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", manifest_path.display()))?;

    merge_layers_recursive(&mut root, project_dir)?;

    let local_path = project_dir.join("scarlet.local.toml");
    if local_path.exists() {
        let local_text = fs::read_to_string(&local_path)
            .map_err(|e| format!("failed to read {}: {e}", local_path.display()))?;
        let local_value: toml::Value = toml::from_str(&local_text)
            .map_err(|e| format!("failed to parse {}: {e}", local_path.display()))?;
        merge_toml_into(&mut root, local_value);
        eprintln!("cargo-scarlet: applied overrides from scarlet.local.toml");
    }

    let merged_text = toml::to_string(&root)
        .map_err(|e| format!("failed to re-serialize merged manifest: {e}"))?;
    let manifest: ScarletManifest =
        toml::from_str(&merged_text).map_err(|e| format!("failed to deserialize manifest: {e}"))?;

    if manifest.schema_version != 1 {
        return Err(format!(
            "unsupported schema_version {} (expected 1)",
            manifest.schema_version
        ));
    }

    Ok(manifest)
}

fn merge_toml_into(parent: &mut toml::Value, child: toml::Value) {
    let toml::Value::Table(parent_table) = parent else {
        return;
    };
    let toml::Value::Table(child_table) = child else {
        return;
    };
    for (key, child_val) in child_table {
        match parent_table.get_mut(&key) {
            Some(toml::Value::Array(parent_arr)) => {
                if let toml::Value::Array(child_arr) = child_val {
                    parent_arr.extend(child_arr);
                }
            }
            Some(parent_existing) => {
                let child_tables = matches!(parent_existing, toml::Value::Table(_))
                    && matches!(&child_val, toml::Value::Table(_));
                if child_tables {
                    let mut taken = parent_existing.clone();
                    merge_toml_into(&mut taken, child_val);
                    parent_table.insert(key, taken);
                }
            }
            _ => {
                parent_table.insert(key, child_val);
            }
        }
    }
}

fn resolve_toml_paths(value: &mut toml::Value, origin_dir: &Path) {
    let toml::Value::Table(table) = value else {
        return;
    };
    for (key, val) in table.iter_mut() {
        match val {
            toml::Value::String(s) if key == "source" || key == "path" => {
                let resolved = resolve_path(origin_dir, s);
                *s = resolved.to_string_lossy().to_string();
            }
            toml::Value::Table(_) => resolve_toml_paths(val, origin_dir),
            toml::Value::Array(arr) => {
                for item in arr.iter_mut() {
                    resolve_toml_paths(item, origin_dir);
                }
            }
            _ => {}
        }
    }
}

fn merge_layers_recursive(value: &mut toml::Value, base_dir: &Path) -> Result<(), String> {
    let layers = match value {
        toml::Value::Table(table) => table.remove("layers"),
        _ => None,
    };

    if let Some(toml::Value::Array(layers)) = layers {
        for layer_entry in layers {
            let toml::Value::Table(layer_tbl) = layer_entry else {
                continue;
            };
            let Some(toml::Value::String(path_str)) = layer_tbl.get("path") else {
                continue;
            };
            let layer_path = resolve_path(base_dir, path_str);
            let layer_dir = layer_path.parent().unwrap_or(Path::new("."));
            let layer_text = fs::read_to_string(&layer_path)
                .map_err(|e| format!("failed to read layer {}: {e}", layer_path.display()))?;
            let mut layer_value: toml::Value = toml::from_str(&layer_text)
                .map_err(|e| format!("failed to parse layer {}: {e}", layer_path.display()))?;
            merge_layers_recursive(&mut layer_value, layer_dir)?;
            resolve_toml_paths(&mut layer_value, layer_dir);
            merge_toml_into(value, layer_value);
        }
    }

    if let toml::Value::Table(table) = value {
        for (_key, sub_value) in table.iter_mut() {
            if let toml::Value::Table(_sub_table) = sub_value {
                let sub_dir = base_dir.to_path_buf();
                merge_layers_recursive(sub_value, &sub_dir)?;
            }
        }
    }

    Ok(())
}

fn resolve_package(
    pkg: &ManifestPackage,
    base_dir: &Path,
    target_triple: &str,
    images: &BTreeMap<String, ManifestImageSection>,
) -> ResolvedPackage {
    let arch = target_triple.split('-').next().unwrap_or("unknown");
    let source = pkg.source.as_ref().map(|s| match s {
        PackageSource::Path(p) => {
            let expanded = expand_templates(p, target_triple, arch);
            PackageSource::Path(expanded)
        }
        PackageSource::Git {
            git,
            branch,
            tag,
            rev,
        } => PackageSource::Git {
            git: git.clone(),
            branch: branch.clone(),
            tag: tag.clone(),
            rev: rev.clone(),
        },
    });
    let local_source = source.as_ref().and_then(|s| s.to_local_path(base_dir));
    ResolvedPackage {
        kind: pkg.kind.clone(),
        source,
        local_source,
        resolved_rev: None,
        package_name: pkg.package.clone(),
        bin: pkg.bin.clone(),
        from: pkg.from.as_ref().and_then(|s| {
            if images.contains_key(s.as_str()) {
                None
            } else {
                Some(resolve_path(
                    base_dir,
                    &expand_templates(s, target_triple, arch),
                ))
            }
        }),
        from_image: pkg
            .from
            .as_ref()
            .filter(|s| images.contains_key(s.as_str()))
            .cloned(),
        to: expand_templates(&pkg.to, target_triple, arch),
        output: pkg.output.as_ref().map(|o| resolve_path(base_dir, o)),
    }
}

fn resolve_path(base: &Path, relative: &str) -> PathBuf {
    let path = Path::new(relative);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn git_resolve_rev(url: &str, refspec: &str) -> Result<String, String> {
    let output = Command::new("git")
        .arg("ls-remote")
        .arg(url)
        .arg(refspec)
        .output()
        .map_err(|e| format!("failed to run git ls-remote: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git ls-remote failed for {url}: {stderr}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .next()
        .ok_or_else(|| format!("git ls-remote returned no output for {url} {refspec}"))?;
    let rev = line
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("git ls-remote unexpected output: {line}"))?;
    if rev.len() < 40 {
        return Err(format!("git ls-remote unexpected rev: {rev}"));
    }
    Ok(rev[..40].to_string())
}

fn git_cache_dir_for_url(url: &str, cache_base: &Path) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = hex::encode(hasher.finalize());
    cache_base.join(&hash[..16])
}

fn git_ensure_checkout(url: &str, rev: &str, cache_base: &Path) -> Result<PathBuf, String> {
    let dir = git_cache_dir_for_url(url, cache_base);
    if dir.join(".git").exists() {
        let head_rev = git_current_rev(&dir)?;
        if head_rev == rev {
            return Ok(dir);
        }
        let status = Command::new("git")
            .arg("fetch")
            .arg("origin")
            .current_dir(&dir)
            .status()
            .map_err(|e| format!("git fetch failed: {e}"))?;
        if !status.success() {
            return Err(format!("git fetch failed in {}", dir.display()));
        }
    } else {
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("failed to create cache dir: {e}"))?;
        }
        let status = Command::new("git")
            .arg("clone")
            .arg(url)
            .arg(&dir)
            .status()
            .map_err(|e| format!("git clone failed: {e}"))?;
        if !status.success() {
            return Err(format!("git clone failed for {url}"));
        }
    }
    let status = Command::new("git")
        .arg("checkout")
        .arg(rev)
        .current_dir(&dir)
        .status()
        .map_err(|e| format!("git checkout failed: {e}"))?;
    if !status.success() {
        return Err(format!("git checkout {rev} failed in {}", dir.display()));
    }
    Ok(dir)
}

fn git_current_rev(dir: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git rev-parse: {e}"))?;
    if !output.status.success() {
        return Err(format!("git rev-parse failed in {}", dir.display()));
    }
    let rev = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if rev.len() < 40 {
        return Err(format!("git rev-parse unexpected output: {rev}"));
    }
    Ok(rev[..40].to_string())
}

fn resolve_git_sources(
    expanded: &mut ExpandedManifest,
    project: &Path,
    existing_lock: &ImageLock,
) -> Result<(), String> {
    let cache_dir = project.join(".scarlet/cache/git");
    for (section_name, section) in expanded.sections.iter_mut() {
        for pkg in &mut section.packages {
            if let Some(PackageSource::Git {
                git,
                branch,
                tag,
                rev,
                ..
            }) = &pkg.source
            {
                if pkg.local_source.is_some() {
                    continue;
                }
                let url = git.clone();

                let locked_rev = existing_lock
                    .sections
                    .get(section_name)
                    .and_then(|s| {
                        s.packages
                            .iter()
                            .find(|p| p.git.as_deref() == Some(&url) && p.bin == pkg.bin)
                    })
                    .and_then(|p| p.resolved_rev.clone());

                let resolved_rev = if let Some(rev) = locked_rev {
                    eprintln!("cargo-scarlet: using locked {} -> {rev}", url);
                    rev
                } else {
                    let refspec = rev
                        .as_deref()
                        .or(tag.as_deref())
                        .or(branch.as_deref())
                        .unwrap_or("HEAD")
                        .to_string();
                    let r = git_resolve_rev(&url, &refspec)?;
                    eprintln!("cargo-scarlet: resolved {} {} -> {r}", url, refspec);
                    r
                };
                let checkout_dir = git_ensure_checkout(&url, &resolved_rev, &cache_dir)?;
                pkg.local_source = Some(checkout_dir);
                pkg.resolved_rev = Some(resolved_rev);
            }
        }
    }
    Ok(())
}

struct TemplateContext {
    arch: String,
    target_triple: String,
    project: String,
}

impl TemplateContext {
    fn expand(&self, s: &str) -> String {
        s.replace("{target_triple}", &self.target_triple)
            .replace("{arch}", &self.arch)
            .replace("{project}", &self.project)
    }
}

fn expand_templates(s: &str, target_triple: &str, arch: &str) -> String {
    s.replace("{target_triple}", target_triple)
        .replace("{arch}", arch)
}

fn userspace_target_triple(kernel_triple: &str) -> String {
    let arch = kernel_triple.split('-').next().unwrap_or("unknown");
    match arch {
        "aarch64" => "aarch64-unknown-scarlet".to_string(),
        v if v.starts_with("riscv64") => "riscv64gc-unknown-scarlet".to_string(),
        _ => kernel_triple.to_string(),
    }
}

#[derive(Debug)]
enum FileSource {
    Local(PathBuf),
    Url(String),
}

struct ResolvedFile {
    source: FileSource,
    to: String,
    template: bool,
}

fn resolve_section(
    section: &ManifestImageSection,
    base_dir: &Path,
    ctx: &TemplateContext,
    images: &BTreeMap<String, ManifestImageSection>,
) -> Result<ResolvedSection, String> {
    let mut packages = Vec::new();
    let mut files = Vec::new();

    for pkg in &section.packages {
        packages.push(resolve_package(pkg, base_dir, &ctx.target_triple, images));
    }

    for f in &section.files {
        let source = if f.source.starts_with("https://") || f.source.starts_with("http://") {
            FileSource::Url(f.source.clone())
        } else {
            FileSource::Local(resolve_path(base_dir, &ctx.expand(&f.source)))
        };
        files.push(ResolvedFile {
            source,
            to: ctx.expand(&f.to),
            template: f.template,
        });
    }

    Ok(ResolvedSection { packages, files })
}

fn expand_manifest(project_dir: &Path) -> Result<ExpandedManifest, String> {
    let manifest = load_manifest(project_dir)?;
    let target_triple = manifest.kernel.target.clone();
    let raw_arch = target_triple.split('-').next().unwrap_or("unknown");
    let arch = match raw_arch {
        "riscv64gc" => "riscv64".to_string(),
        other => other.to_string(),
    };
    let project = manifest.project.name.clone();

    let ctx = TemplateContext {
        arch,
        target_triple,
        project,
    };

    let mut sections = BTreeMap::new();
    let images_ref = &manifest.images;
    for (name, section) in images_ref {
        sections.insert(
            name.clone(),
            resolve_section(section, project_dir, &ctx, images_ref)?,
        );
    }

    Ok(ExpandedManifest {
        project_dir: project_dir.to_path_buf(),
        manifest,
        sections,
    })
}

fn generate_from_manifest(project_dir: &Path) -> Result<ExpandedManifest, String> {
    let expanded = expand_manifest(project_dir)?;

    let generated_root = project_dir.join(".scarlet/scarlet-modules");
    let generated_src = generated_root.join("src");
    let generated_cargo = generated_root.join(".cargo");
    fs::create_dir_all(&generated_src)
        .map_err(|e| format!("failed to create {}: {e}", generated_src.display()))?;
    fs::create_dir_all(&generated_cargo)
        .map_err(|e| format!("failed to create {}: {e}", generated_cargo.display()))?;

    let cargo_toml = render_manifest_cargo_toml(&expanded.manifest, project_dir)?;
    let lib_rs = render_manifest_lib_rs(&expanded.manifest);

    write_if_changed(&generated_root.join("Cargo.toml"), &cargo_toml)?;
    write_if_changed(&generated_src.join("lib.rs"), &lib_rs)?;

    let cargo_config_path = generated_cargo.join("config.toml");
    if !cargo_config_path.exists() {
        let cargo_config = render_cargo_config();
        fs::write(&cargo_config_path, cargo_config)
            .map_err(|e| format!("failed to write {}: {e}", cargo_config_path.display()))?;
    }

    Ok(expanded)
}

fn render_manifest_cargo_toml(
    manifest: &ScarletManifest,
    project_dir: &Path,
) -> Result<String, String> {
    let mut out = String::new();
    let _ = writeln!(&mut out, "# generated by cargo-scarlet");
    out.push_str("[package]\n");
    out.push_str("name = \"scarlet-modules\"\n");
    out.push_str("version = \"0.1.0\"\n");
    out.push_str("edition = \"2024\"\n\n");
    out.push_str("[lib]\npath = \"src/lib.rs\"\n\n");
    out.push_str("[dependencies]\n");

    let features = render_enabled_kernel_features(&manifest.kernel.features);
    let kernel_dep = match &manifest.kernel.source {
        PackageSource::Path(p) => {
            let expanded = expand_templates(p, &manifest.kernel.target, "");
            let kernel_abs = resolve_path(project_dir, &expanded);
            let generated_root = project_dir.join(".scarlet/scarlet-modules");
            let kernel_rel = pathdiff(&kernel_abs, &generated_root)?;
            format!(
                "{{ path = \"{}\", default-features = false, features = [{}] }}",
                kernel_rel.display(),
                features
            )
        }
        PackageSource::Git {
            git,
            branch,
            tag,
            rev,
        } => {
            let mut parts = vec![format!("git = \"{git}\"")];
            if let Some(r) = rev {
                parts.push(format!("rev = \"{r}\""));
            }
            if let Some(b) = branch {
                parts.push(format!("branch = \"{b}\""));
            }
            if let Some(t) = tag {
                parts.push(format!("tag = \"{t}\""));
            }
            parts.push("default-features = false".to_string());
            parts.push(format!("features = [{features}]"));
            format!("{{ {} }}", parts.join(", "))
        }
    };
    let _ = writeln!(&mut out, "{} = {}", manifest.kernel.package, kernel_dep);

    for (name, module) in &manifest.modules {
        if !module.enabled {
            continue;
        }
        let spec = render_dependency_spec(project_dir, module)?;
        let _ = writeln!(&mut out, "{name} = {{ {spec} }}");
    }

    Ok(out)
}

fn render_manifest_lib_rs(manifest: &ScarletManifest) -> String {
    let mut source = String::new();
    source.push_str("#![no_std]\n\n");
    source.push_str("pub use scarlet;\n\n");
    source.push_str("#[inline(never)]\n");
    source.push_str("pub fn force_link() {\n");
    for name in manifest
        .modules
        .keys()
        .filter(|n| manifest.modules[*n].enabled)
    {
        let identifier = cargo_key_to_rust_identifier(name);
        let _ = writeln!(&mut source, "    {identifier}::force_link();");
    }
    source.push_str("}\n");
    source
}

fn render_cargo_config() -> String {
    let mut out = String::new();
    out.push_str("# Configure build settings for the scarlet-modules workspace.\n");
    out.push_str("# This file is generated once by cargo-scarlet and will not be overwritten.\n");
    out.push_str("#\n");
    out.push_str("# Required fields:\n");
    out.push_str("#   [build]\n");
    out.push_str("#   target = \"<path-to-target-json>\"\n");
    out.push_str("#\n");
    out.push_str("#   [unstable]\n");
    out.push_str("#   build-std = [\"core\", \"compiler_builtins\", \"alloc\"]\n");
    out.push_str("#   build-std-features = [\"compiler-builtins-mem\"]\n");
    out.push_str("#\n");
    out.push_str("# Optional:\n");
    out.push_str("#   [profile.dev]\n");
    out.push_str("#   opt-level = 3\n");
    out.push_str("#\n");
    out.push_str("#   [target.<target-triple>]\n");
    out.push_str("#   rustflags = [\"-T\", \"path/to/linker.ld\"]\n");
    out.push('\n');
    out
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut f = fs::File::open(path)
        .map_err(|e| format!("failed to open {} for hashing: {e}", path.display()))?;
    let mut buf = [0u8; 8192];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn sha256_dir(dir: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    sha256_dir_recursive(dir, &mut hasher)?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn load_lock(project_dir: &Path) -> ImageLock {
    let lock_path = project_dir.join("scarlet.lock");
    let text = match fs::read_to_string(&lock_path) {
        Ok(t) => t,
        Err(_) => return ImageLock::default(),
    };
    let image_lock: ImageLock = match toml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return ImageLock::default(),
    };
    image_lock
}

fn save_lock(project_dir: &Path, lock: &ImageLock) -> Result<(), String> {
    let lock_path = project_dir.join("scarlet.lock");
    let mut text = String::from("# Generated by cargo-scarlet — do not edit\n\n");
    let lock_toml =
        toml::to_string_pretty(lock).map_err(|e| format!("failed to serialize lock: {e}"))?;
    text.push_str(&lock_toml);
    fs::write(&lock_path, &text)
        .map_err(|e| format!("failed to write {}: {e}", lock_path.display()))?;
    eprintln!("cargo-scarlet: wrote {}", lock_path.display());
    Ok(())
}

fn sha256_dir_recursive(dir: &Path, hasher: &mut Sha256) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("failed to read_dir {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_symlink() {
            let target = fs::read_link(&path)
                .map_err(|e| format!("failed to read symlink {}: {e}", path.display()))?;
            hasher.update(
                format!("sym:{}:{}\n", entry.file_name().display(), target.display()).as_bytes(),
            );
        } else if path.is_dir() {
            hasher.update(format!("dir:{}\n", entry.file_name().display()).as_bytes());
            sha256_dir_recursive(&path, hasher)?;
        } else {
            let mut f = fs::File::open(&path)
                .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
            let mut content = Vec::new();
            f.read_to_end(&mut content)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            let file_hash = format!("{:x}", sha2::Sha256::digest(&content));
            hasher
                .update(format!("file:{}:{}\n", entry.file_name().display(), file_hash).as_bytes());
        }
    }
    Ok(())
}

fn cmd_update(project: &Path) -> Result<(), String> {
    let mut expanded = expand_manifest(project)?;
    let git_cache_dir = project.join(".scarlet/cache/git");
    let file_cache_dir = project.join(".scarlet/cache/files");
    let mut lock = load_lock(project);

    for section in expanded.sections.values_mut() {
        for pkg in &mut section.packages {
            if let Some(ref src) = pkg.source
                && src.is_git()
            {
                let url = src.git_url().unwrap();
                let refspec = src.git_ref().unwrap_or_else(|| "HEAD".to_string());
                let rev = git_resolve_rev(url, &refspec)?;
                eprintln!("cargo-scarlet: resolved {} {} -> {rev}", url, refspec);
                let checkout = git_ensure_checkout(url, &rev, &git_cache_dir)?;
                pkg.local_source = Some(checkout);
                pkg.resolved_rev = Some(rev.clone());
            }
        }
    }

    for (section_name, section) in &expanded.sections {
        let section_lock = lock
            .sections
            .entry(section_name.clone())
            .or_insert_with(|| SectionLock {
                hash: String::new(),
                files: Vec::new(),
                packages: Vec::new(),
            });

        for pkg in &section.packages {
            if pkg.source.as_ref().is_some_and(|s| s.is_git()) {
                let source = pkg
                    .local_source
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string());
                let git = pkg
                    .source
                    .as_ref()
                    .and_then(|s| s.git_url())
                    .map(|s| s.to_string());
                let new_pkg = PackageLock {
                    kind: pkg.kind.clone().unwrap_or_default(),
                    source,
                    git: git.clone(),
                    resolved_rev: pkg.resolved_rev.clone(),
                    bin: pkg.bin.clone(),
                    output: None,
                    hash: String::new(),
                };
                if let Some(existing) = section_lock
                    .packages
                    .iter_mut()
                    .find(|p| p.git == git && p.bin == new_pkg.bin)
                {
                    existing.resolved_rev = new_pkg.resolved_rev;
                    existing.source = new_pkg.source;
                } else {
                    section_lock.packages.push(new_pkg);
                }
            }
        }

        for file in &section.files {
            if let FileSource::Url(url) = &file.source {
                eprintln!("cargo-scarlet: fetching {}", url);
                let (_, hash) = fetch_url_cached(url, &file_cache_dir, None)?;
                if let Some(existing) = section_lock.files.iter_mut().find(|f| f.source == *url) {
                    existing.hash = hash;
                } else {
                    section_lock.files.push(FileLock {
                        source: url.clone(),
                        hash,
                    });
                }
            }
        }
    }

    let section_names: Vec<String> = expanded.sections.keys().cloned().collect();
    lock.sections.retain(|name, _| section_names.contains(name));

    save_lock(project, &lock)?;
    eprintln!("cargo-scarlet: lock updated");
    Ok(())
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
            let project = normalize_project_path(&project)?;
            let expanded = generate_from_manifest(&project)?;
            cargo_build_manifest(
                &project,
                &expanded,
                target.as_deref(),
                release,
                "check",
                &[],
            )
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
                let project = normalize_project_path(&project)?;
                let expanded = generate_from_manifest(&project)?;
                cargo_build_manifest(
                    &project,
                    &expanded,
                    target.as_deref(),
                    release,
                    "build",
                    &[],
                )?;
                inject_ksym_section_manifest(&project, &expanded, target.as_deref(), release)
            }
        }
        Commands::Clippy {
            project,
            target,
            release,
            extra_args,
        } => {
            let project = normalize_project_path(&project)?;
            let expanded = generate_from_manifest(&project)?;
            cargo_build_manifest(
                &project,
                &expanded,
                target.as_deref(),
                release,
                "clippy",
                &extra_args,
            )
        }
        Commands::Run {
            project,
            target,
            release,
            no_image,
            extra_args,
        } => {
            let project = normalize_project_path(&project)?;
            let expanded = generate_from_manifest(&project)?;

            if !no_image {
                build_manifest_image(&project, target, release, None, false)?;
            }

            match &expanded.manifest.runner {
                Some(runner) => {
                    let runner_path = if Path::new(&runner.command).is_absolute() {
                        PathBuf::from(&runner.command)
                    } else {
                        project.join(&runner.command)
                    };

                    let mut cmd = Command::new(&runner_path);
                    cmd.current_dir(&project);
                    if release {
                        cmd.env("SCARLET_RELEASE", "1");
                    }
                    cmd.args(&extra_args);

                    let status = cmd
                        .status()
                        .map_err(|e| format!("failed to run runner: {e}"))?;

                    if status.success() {
                        Ok(())
                    } else {
                        Err("runner exited with non-zero status".to_string())
                    }
                }
                None => {
                    Err("no [runner] defined in scarlet.toml; running is not supported for this project".to_string())
                }
            }
        }
        Commands::Image {
            project,
            target,
            release,
            kernel_elf,
            no_build,
        } => {
            let project = normalize_project_path(&project)?;
            build_manifest_image(&project, target, release, kernel_elf, no_build)
        }
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
        Commands::Update { project } => {
            let project = normalize_project_path(&project)?;
            cmd_update(&project)
        }
    }
}

fn cargo_build_manifest(
    project: &Path,
    expanded: &ExpandedManifest,
    target: Option<&str>,
    release: bool,
    subcommand: &str,
    extra_args: &[String],
) -> Result<(), String> {
    let resolved_target = target
        .map(str::to_string)
        .unwrap_or_else(|| expanded.manifest.kernel.target_json.clone());

    metadata_check(project, &resolved_target)?;

    let mut command = Command::new("cargo");
    command.arg(subcommand);
    if release {
        command.arg("--release");
    }
    command.arg("--target").arg(&resolved_target);

    if subcommand == "clippy" && !extra_args.iter().any(|arg| arg == "--") {
        command.arg("--").arg("-D").arg("warnings");
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
        .map_err(|e| format!("failed to run cargo {subcommand}: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {subcommand} failed with status {status}"))
    }
}

fn inject_ksym_section_manifest(
    project: &Path,
    expanded: &ExpandedManifest,
    target: Option<&str>,
    release: bool,
) -> Result<(), String> {
    let resolved_target = match target {
        Some(t) => t.to_string(),
        None => expanded.manifest.kernel.target_json.clone(),
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

    let objcopy_status = Command::new(&objcopy_cmd)
        .args([
            "--add-section",
            &format!(".ksym={}", blob_path.display()),
            "--set-section-flags",
            ".ksym=alloc,readonly",
            binary_path.to_str().unwrap_or(""),
        ])
        .status()
        .map_err(|e| format!("failed to run objcopy: {e}"))?;

    if objcopy_status.success() {
        eprintln!("cargo-scarlet: ksym: injected {} symbols", symbols.len());
        Ok(())
    } else {
        eprintln!("cargo-scarlet: ksym: objcopy failed, skipping section injection");
        Ok(())
    }
}

fn build_manifest_image(
    project: &Path,
    target: Option<String>,
    release: bool,
    kernel_elf: Option<PathBuf>,
    no_build: bool,
) -> Result<(), String> {
    let mut expanded = generate_from_manifest(project)?;

    if !no_build {
        cargo_build_manifest(project, &expanded, target.as_deref(), release, "build", &[])?;
        inject_ksym_section_manifest(project, &expanded, target.as_deref(), release)?;
    }

    let kernel_elf = match kernel_elf {
        Some(path) => absolutize_from_current_dir(&path)?,
        None => {
            let target_json = &expanded.manifest.kernel.target_json;
            let target_path = if Path::new(target_json).is_absolute() {
                PathBuf::from(target_json)
            } else {
                project.join(target_json)
            };
            let target_triple = target_path
                .file_stem()
                .ok_or("target path has no file stem")?
                .to_string_lossy()
                .to_string();
            let profile = if release { "release" } else { "debug" };
            let path = project
                .join("target")
                .join(&target_triple)
                .join(profile)
                .join("scarlet");
            if !path.exists() {
                return Err(format!("kernel ELF not found: {}", path.display()));
            }
            path
        }
    };

    let images_dir = project.join(".scarlet/images");
    fs::create_dir_all(&images_dir)
        .map_err(|e| format!("failed to create {}: {e}", images_dir.display()))?;

    let target_json = &expanded.manifest.kernel.target_json;
    let target_path = if Path::new(target_json).is_absolute() {
        PathBuf::from(target_json)
    } else {
        project.join(target_json)
    };
    let target_triple = target_path
        .file_stem()
        .ok_or("target path has no file stem")?
        .to_string_lossy()
        .to_string();
    let profile = if release { "release" } else { "debug" };

    let raw_arch = target_triple.split('-').next().unwrap_or("unknown");
    let arch = match raw_arch {
        v if v.starts_with("riscv64") => "riscv64".to_string(),
        v if v.starts_with("riscv32") => "riscv32".to_string(),
        v if v.starts_with("aarch64") => "aarch64".to_string(),
        v => v.to_string(),
    };
    let tpl_ctx = TemplateContext {
        arch,
        target_triple: target_triple.clone(),
        project: expanded.manifest.project.name.clone(),
    };

    let build_order = topo_sort_images(&expanded.manifest.images)?;

    let existing_lock = load_lock(project);
    resolve_git_sources(&mut expanded, project, &existing_lock)?;
    let mut new_lock = ImageLock::default();

    for section_name in build_order {
        let section_cfg = expanded
            .manifest
            .images
            .get(&section_name)
            .ok_or_else(|| format!("section '{}' not in manifest", section_name))?;
        let resolved = expanded
            .sections
            .get(&section_name)
            .ok_or_else(|| format!("section '{}' not resolved", section_name))?;

        let output = section_cfg
            .output
            .as_deref()
            .unwrap_or(".scarlet/images/output");
        let output_path = project.join(output);
        let staging_dir = project.join(format!(".scarlet/staging/{}", section_name));
        let format = section_cfg.format.as_deref().unwrap_or("");

        eprintln!("cargo-scarlet: staging {}...", section_name);

        match format {
            "newc" | "ext2" => {
                if staging_dir.exists() {
                    fs::remove_dir_all(&staging_dir)
                        .map_err(|e| format!("failed to clean staging: {e}"))?;
                }
                fs::create_dir_all(&staging_dir)
                    .map_err(|e| format!("failed to create staging: {e}"))?;

                let cache_dir = project.join(".scarlet/cache/files");
                let prev_section_lock = existing_lock.sections.get(&section_name);
                let mut file_locks: Vec<FileLock> = Vec::new();

                for file in &resolved.files {
                    let local_path = match &file.source {
                        FileSource::Local(p) => p.clone(),
                        FileSource::Url(u) => {
                            let expected = prev_section_lock.and_then(|s| {
                                s.files
                                    .iter()
                                    .find(|f| f.source == *u)
                                    .map(|f| f.hash.as_str())
                            });
                            let (path, hash) = fetch_url_cached(u, &cache_dir, expected)?;
                            file_locks.push(FileLock {
                                source: u.clone(),
                                hash,
                            });
                            path
                        }
                    };

                    let dest = staging_dir.join(file.to.trim_start_matches('/'));
                    if local_path.is_dir() {
                        copy_dir_recursive(&local_path, &dest)?;
                    } else if file.template {
                        if let Some(parent) = dest.parent() {
                            fs::create_dir_all(parent).map_err(|e| {
                                format!("failed to create {}: {e}", parent.display())
                            })?;
                        }
                        let content = fs::read_to_string(&local_path).map_err(|e| {
                            format!("failed to read template {}: {e}", local_path.display())
                        })?;
                        let expanded = tpl_ctx.expand(&content);
                        fs::write(&dest, expanded).map_err(|e| {
                            format!(
                                "failed to write template {} -> {}: {e}",
                                local_path.display(),
                                dest.display()
                            )
                        })?;
                    } else {
                        if let Some(parent) = dest.parent() {
                            fs::create_dir_all(parent).map_err(|e| {
                                format!("failed to create {}: {e}", parent.display())
                            })?;
                        }
                        fs::copy(&local_path, &dest).map_err(|e| {
                            format!(
                                "failed to copy {} -> {}: {e}",
                                local_path.display(),
                                dest.display()
                            )
                        })?;
                    }
                }

                let mut package_locks: Vec<PackageLock> = Vec::new();
                for pkg in &resolved.packages {
                    if let Some(ref img_name) = pkg.from_image {
                        let from_staging = project.join(format!(".scarlet/staging/{}", img_name));
                        let dest = staging_dir.join(pkg.to.trim_start_matches('/'));
                        if from_staging.is_dir() {
                            copy_dir_recursive(&from_staging, &dest)?;
                        }
                    } else {
                        let prev_pkg = existing_lock.sections.get(&section_name).and_then(|s| {
                            s.packages.iter().find(|p| {
                                p.kind == pkg.kind.as_deref().unwrap_or("")
                                    && p.source.as_deref()
                                        == pkg
                                            .local_source
                                            .as_ref()
                                            .map(|s| s.to_string_lossy().to_string())
                                            .as_deref()
                            })
                        });
                        if let Some(lock) = install_package(
                            &staging_dir,
                            pkg,
                            project,
                            &target_triple,
                            profile,
                            prev_pkg,
                        )? {
                            package_locks.push(lock);
                        }
                    }
                }

                let staging_hash = sha256_dir(&staging_dir)?;

                if output_path.exists()
                    && let Some(existing) = existing_lock.sections.get(&section_name)
                    && existing.hash == staging_hash
                {
                    eprintln!(
                        "cargo-scarlet: {} unchanged, skipping image generation",
                        section_name
                    );
                    let mut updated = existing.clone();
                    updated.packages = package_locks;
                    updated.files = file_locks;
                    new_lock.sections.insert(section_name.clone(), updated);
                    continue;
                }

                eprintln!("cargo-scarlet: generating {} image...", section_name);

                if format == "newc" {
                    build_initramfs_newc_from_staging(&staging_dir, &output_path)?;
                    eprintln!(
                        "cargo-scarlet: wrote {} to {}",
                        section_name,
                        output_path.display()
                    );
                } else {
                    build_ext2_from_staging(&staging_dir, &output_path, &section_name)?;
                }

                new_lock.sections.insert(
                    section_name.clone(),
                    SectionLock {
                        hash: staging_hash,
                        files: file_locks,
                        packages: package_locks,
                    },
                );
            }
            "limine-uefi" => {
                let arch_name = match target_triple.split('-').next() {
                    Some("aarch64") => "aarch64",
                    Some(v) if v.starts_with("riscv64") => "riscv64",
                    _ => &target_triple,
                };

                let mut initramfs_path = project.join(".scarlet/images/initramfs.cpio");
                for pkg in &resolved.packages {
                    if let Some(source) = &pkg.local_source {
                        let source_name = source
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        if source_name.contains("initramfs") || source_name.ends_with(".cpio") {
                            initramfs_path = source.clone();
                        }
                    }
                }

                let mut limine_hasher = Sha256::new();
                limine_hasher.update(b"format=limine-uefi\n");
                limine_hasher.update(format!("cmdline={}\n", section_cfg.cmdline).as_bytes());
                if kernel_elf.exists() {
                    limine_hasher.update(
                        format!(
                            "kernel:{}:{}\n",
                            kernel_elf.display(),
                            sha256_file(&kernel_elf)?
                        )
                        .as_bytes(),
                    );
                }
                if initramfs_path.exists() {
                    limine_hasher.update(
                        format!(
                            "initramfs:{}:{}\n",
                            initramfs_path.display(),
                            sha256_file(&initramfs_path)?
                        )
                        .as_bytes(),
                    );
                }
                let limine_hash = format!("sha256:{}", hex::encode(limine_hasher.finalize()));

                if output_path.exists()
                    && let Some(existing) = existing_lock.sections.get(&section_name)
                    && existing.hash == limine_hash
                {
                    eprintln!("cargo-scarlet: {} unchanged, skipping", section_name);
                    new_lock
                        .sections
                        .insert(section_name.clone(), existing.clone());
                    continue;
                }

                eprintln!("cargo-scarlet: building {}...", section_name);

                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
                }

                let request = PluginRequest {
                    project_dir: project.display().to_string(),
                    section_name: &section_name,
                    format,
                    arch: arch_name.to_string(),
                    kernel_elf: kernel_elf.display().to_string(),
                    initramfs: Some(initramfs_path.display().to_string()),
                    output: output_path.display().to_string(),
                    section: section_cfg,
                };
                run_plugin("limine", &request)?;

                eprintln!(
                    "cargo-scarlet: wrote {} to {}",
                    section_name,
                    output_path.display()
                );

                new_lock.sections.insert(
                    section_name.clone(),
                    SectionLock {
                        hash: limine_hash,
                        files: Vec::new(),
                        packages: Vec::new(),
                    },
                );
            }
            _ => {
                return Err(format!(
                    "unsupported format '{}' for section '{}'",
                    format, section_name
                ));
            }
        }
    }

    eprintln!(
        "cargo-scarlet: saving lock with {} sections",
        new_lock.sections.len()
    );
    save_lock(project, &new_lock)?;

    Ok(())
}

fn run_plugin<T: Serialize>(name: &str, request: &T) -> Result<(), String> {
    let program = format!("cargo-scarlet-plugin-{name}");
    let payload = serde_json::to_vec(request)
        .map_err(|error| format!("failed to encode plugin request for '{name}': {error}"))?;
    let mut child = Command::new(&program)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to run plugin '{name}' ({program}): {error}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| format!("failed to open stdin for plugin '{name}'"))?
        .write_all(&payload)
        .map_err(|error| format!("failed to write plugin request for '{name}': {error}"))?;
    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for plugin '{name}': {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("plugin '{name}' failed with status {status}"))
    }
}

fn topo_sort_images(
    images: &BTreeMap<String, ManifestImageSection>,
) -> Result<Vec<String>, String> {
    let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for name in images.keys() {
        in_degree.insert(name.clone(), 0);
        dependents.insert(name.clone(), Vec::new());
    }

    for (name, section) in images {
        for dep in &section.deps {
            if !images.contains_key(dep) {
                return Err(format!(
                    "image '{}' depends on unknown image '{}'",
                    name, dep
                ));
            }
            *in_degree.entry(name.clone()).or_insert(0) += 1;
            dependents
                .entry(dep.clone())
                .or_default()
                .push(name.clone());
        }
    }

    let mut queue: Vec<String> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| name.clone())
        .collect();

    let mut result = Vec::new();
    while let Some(name) = queue.pop() {
        result.push(name.clone());
        for dep in dependents.get(&name).unwrap_or(&Vec::new()) {
            let degree = in_degree.get_mut(dep).unwrap();
            *degree -= 1;
            if *degree == 0 {
                queue.push(dep.clone());
            }
        }
    }

    if result.len() != images.len() {
        return Err("circular dependency detected in images".to_string());
    }

    Ok(result)
}

fn build_ext2_from_staging(
    staging_dir: &Path,
    output_path: &Path,
    section_name: &str,
) -> Result<(), String> {
    let source_kb_output = Command::new("du")
        .args(["-sk", staging_dir.to_str().unwrap_or("")])
        .output()
        .map_err(|e| format!("failed to run du: {e}"))?;
    let source_kb_str = String::from_utf8_lossy(&source_kb_output.stdout);
    let source_kb: u64 = source_kb_str
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let extra_kb: u64 = 65536;
    let size_kb = (source_kb + source_kb / 3 + extra_kb).div_ceil(16384) * 16384;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }

    let _ = fs::remove_file(output_path);
    let truncate_status = Command::new("truncate")
        .args([
            "-s",
            &format!("{size_kb}K"),
            output_path.to_str().unwrap_or(""),
        ])
        .status()
        .map_err(|e| format!("failed to run truncate: {e}"))?;
    if !truncate_status.success() {
        return Err("truncate failed".to_string());
    }

    let mke2fs_status = Command::new("mke2fs")
        .args([
            "-q",
            "-F",
            "-t",
            "ext2",
            "-b",
            "4096",
            "-i",
            "2048",
            "-m",
            "1",
            "-L",
            "SCARLET_ROOT",
            "-E",
            "no_copy_xattrs",
            "-d",
            staging_dir.to_str().unwrap_or(""),
            output_path.to_str().unwrap_or(""),
        ])
        .status()
        .map_err(|e| format!("failed to run mke2fs: {e}"))?;
    if !mke2fs_status.success() {
        return Err("mke2fs failed".to_string());
    }

    eprintln!(
        "cargo-scarlet: wrote {} to {} ({}KB, source={}KB)",
        section_name,
        output_path.display(),
        size_kb,
        source_kb
    );
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("failed to create {}: {e}", dst.display()))?;
    for entry in
        fs::read_dir(src).map_err(|e| format!("failed to read_dir {}: {e}", src.display()))?
    {
        let entry = entry.map_err(|e| format!("failed to read dir entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_symlink() {
            let link_target = fs::read_link(&src_path)
                .map_err(|e| format!("failed to read symlink {}: {e}", src_path.display()))?;
            if dst_path.exists() {
                fs::remove_file(&dst_path)
                    .map_err(|e| format!("failed to remove {}: {e}", dst_path.display()))?;
            }
            std::os::unix::fs::symlink(&link_target, &dst_path).map_err(|e| {
                format!(
                    "failed to create symlink {} -> {}: {e}",
                    dst_path.display(),
                    link_target.display()
                )
            })?;
        } else if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| {
                format!(
                    "failed to copy {} -> {}: {e}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn fetch_url_cached(
    url: &str,
    cache_dir: &Path,
    expected_hash: Option<&str>,
) -> Result<(PathBuf, String), String> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = format!("{:016x}", hasher.finish());

    let url_path = url.split('?').next().unwrap_or(url);
    let basename = url_path.rsplit('/').next().unwrap_or("download");
    let cached_name = format!("{}-{}", &hash[..12], basename);
    let cached_path = cache_dir.join(&cached_name);

    if !cached_path.exists() {
        fs::create_dir_all(cache_dir)
            .map_err(|e| format!("failed to create cache dir {}: {e}", cache_dir.display()))?;

        eprintln!("cargo-scarlet: fetching {}", url);
        let status = Command::new("curl")
            .args(["-fsSL", "-o", cached_path.to_str().unwrap_or(""), url])
            .status()
            .map_err(|e| format!("failed to run curl: {e}"))?;
        if !status.success() {
            let _ = fs::remove_file(&cached_path);
            return Err(format!("curl failed to fetch {}", url));
        }
    }

    let actual_hash = sha256_file(&cached_path)?;
    if let Some(expected) = expected_hash
        && actual_hash != expected
    {
        return Err(format!(
            "hash mismatch for {}: expected {}, got {}",
            url, expected, actual_hash
        ));
    }

    Ok((cached_path, actual_hash))
}

fn install_package(
    staging_dir: &Path,
    pkg: &ResolvedPackage,
    _project: &Path,
    target_triple: &str,
    profile: &str,
    prev_lock: Option<&PackageLock>,
) -> Result<Option<PackageLock>, String> {
    let dest = staging_dir.join(pkg.to.trim_start_matches('/'));

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }

    match pkg.kind.as_deref() {
        Some("cargo") => {
            let source = pkg
                .local_source
                .as_ref()
                .ok_or("cargo package missing source")?;
            let package_name = pkg.package_name.as_deref().unwrap_or("user-bin");
            let bin_name = pkg.bin.as_deref().unwrap_or(package_name);

            let userspace_triple = userspace_target_triple(target_triple);
            let profile_dir = if profile == "release" {
                "release"
            } else {
                "debug"
            };

            let binary = {
                eprintln!(
                    "cargo-scarlet: building {} ({}) for {}...",
                    package_name, bin_name, userspace_triple
                );
                let mut cmd = Command::new("cargo");
                cmd.arg("build");
                if profile == "release" {
                    cmd.arg("--release");
                }
                cmd.arg("--manifest-path")
                    .arg(source.join("Cargo.toml"))
                    .arg("--target")
                    .arg(&userspace_triple);

                if let Some(bin) = pkg.bin.as_deref() {
                    cmd.arg("--bin").arg(bin);
                }

                let status = cmd
                    .current_dir(source)
                    .status()
                    .map_err(|e| format!("failed to run cargo build: {e}"))?;

                if !status.success() {
                    return Err(format!(
                        "cargo build failed for {} (bin {})",
                        package_name, bin_name
                    ));
                }

                let built = source
                    .join("target")
                    .join(&userspace_triple)
                    .join(profile_dir)
                    .join(bin_name);
                if !built.exists() {
                    return Err(format!(
                        "cargo build succeeded but binary not found: {}",
                        built.display()
                    ));
                }
                built
            };
            fs::copy(&binary, &dest)
                .map_err(|e| format!("failed to copy {}: {e}", binary.display()))?;

            let hash = sha256_file(&binary)?;
            let (git_url, resolved_rev) = match &pkg.source {
                Some(PackageSource::Git { git, .. }) => {
                    (Some(git.clone()), pkg.resolved_rev.clone())
                }
                _ => (None, None),
            };
            Ok(Some(PackageLock {
                kind: "cargo".to_string(),
                source: Some(source.to_string_lossy().to_string()),
                git: git_url,
                resolved_rev,
                bin: Some(bin_name.to_string()),
                output: None,
                hash,
            }))
        }
        Some("script") => {
            let source = pkg
                .local_source
                .as_ref()
                .ok_or("script package missing source")?;
            let script_path = if source.is_absolute() {
                source.clone()
            } else {
                _project.join(source)
            };
            if !script_path.exists() {
                return Err(format!("script not found: {}", script_path.display()));
            }

            let (script_output, copy_dest) = match &pkg.output {
                Some(output) => {
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
                    }
                    (output.clone(), dest.clone())
                }
                None => {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
                    }
                    (dest.clone(), PathBuf::new())
                }
            };

            if let (Some(output_path), Some(prev)) = (&pkg.output, prev_lock) {
                if output_path.exists() {
                    let current_hash = sha256_file(output_path)?;
                    if current_hash == prev.hash {
                        eprintln!(
                            "cargo-scarlet: script output {} unchanged, skipping",
                            output_path.display()
                        );
                    } else {
                        let status = Command::new("sh")
                            .arg(&script_path)
                            .arg(&script_output)
                            .current_dir(_project)
                            .status()
                            .map_err(|e| {
                                format!("failed to run script {}: {e}", script_path.display())
                            })?;
                        if !status.success() {
                            return Err(format!("script failed: {}", script_path.display()));
                        }
                    }
                } else {
                    let status = Command::new("sh")
                        .arg(&script_path)
                        .arg(&script_output)
                        .current_dir(_project)
                        .status()
                        .map_err(|e| {
                            format!("failed to run script {}: {e}", script_path.display())
                        })?;
                    if !status.success() {
                        return Err(format!("script failed: {}", script_path.display()));
                    }
                }
            } else {
                let status = Command::new("sh")
                    .arg(&script_path)
                    .arg(&script_output)
                    .current_dir(_project)
                    .status()
                    .map_err(|e| format!("failed to run script {}: {e}", script_path.display()))?;
                if !status.success() {
                    return Err(format!("script failed: {}", script_path.display()));
                }
            }

            if !copy_dest.as_os_str().is_empty() {
                if script_output.is_dir() {
                    copy_dir_recursive(&script_output, &copy_dest)?;
                } else if script_output.exists() {
                    if let Some(parent) = copy_dest.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
                    }
                    fs::copy(&script_output, &copy_dest).map_err(|e| {
                        format!(
                            "failed to copy {} -> {}: {e}",
                            script_output.display(),
                            copy_dest.display()
                        )
                    })?;
                }
            }

            let hash = if script_output.is_dir() {
                sha256_dir(&script_output)?
            } else if script_output.exists() {
                sha256_file(&script_output)?
            } else {
                "missing".to_string()
            };
            Ok(Some(PackageLock {
                kind: "script".to_string(),
                source: Some(source.to_string_lossy().to_string()),
                git: None,
                resolved_rev: None,
                bin: None,
                output: pkg.output.as_ref().map(|p| p.to_string_lossy().to_string()),
                hash,
            }))
        }
        _ => {
            let from = pkg.from.as_ref().ok_or_else(|| {
                format!(
                    "package (to={}) missing 'from' path and has unknown kind {:?}",
                    pkg.to, pkg.kind
                )
            })?;
            if from.is_dir() {
                copy_dir_contents(from, &dest, &[])?;
            } else if from.exists() {
                fs::copy(from, &dest)
                    .map_err(|e| format!("failed to copy {}: {e}", from.display()))?;
            } else {
                eprintln!(
                    "cargo-scarlet: warning: source not found: {}",
                    from.display()
                );
            }
            Ok(None)
        }
    }
}

fn build_initramfs_newc_from_staging(staging_dir: &Path, output_path: &Path) -> Result<(), String> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }

    let mut output_file = fs::File::create(output_path)
        .map_err(|e| format!("failed to create {}: {e}", output_path.display()))?;

    write_newc_tree(&mut output_file, staging_dir, staging_dir)?;
    write_newc_trailer(&mut output_file)?;

    Ok(())
}

fn write_newc_trailer(output: &mut fs::File) -> Result<(), String> {
    use std::io::Write;
    let trailer_name = "TRAILER!!!";
    let name_size = trailer_name.len() + 1;
    let header = format!(
        "070701{ino:08x}{mode:08x}{uid:08x}{gid:08x}{nlink:08x}{mtime:08x}{file_size:08x}{dev_major:08x}{dev_minor:08x}{rdev_major:08x}{rdev_minor:08x}{name_size:08x}{check:08x}",
        ino = 0,
        mode = 0,
        uid = 0,
        gid = 0,
        nlink = 1,
        mtime = 0,
        file_size = 0,
        dev_major = 0,
        dev_minor = 0,
        rdev_major = 0,
        rdev_minor = 0,
        name_size = name_size,
        check = 0,
    );
    output
        .write_all(header.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;
    output
        .write_all(trailer_name.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;
    output
        .write_all(&[0])
        .map_err(|e| format!("write failed: {e}"))?;
    pad4(output, 110 + name_size)?;
    Ok(())
}

fn normalized_args() -> Vec<String> {
    let mut args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|arg| arg == "scarlet") {
        args.remove(1);
    }
    args
}

fn normalize_project_path(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("failed to resolve {}: {error}", path.display()))
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

fn write_if_changed(path: &Path, contents: &str) -> Result<(), String> {
    if let Ok(existing) = fs::read_to_string(path)
        && existing == contents
    {
        return Ok(());
    }

    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
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

fn absolutize_from_current_dir(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| format!("failed to get current directory: {error}"))?
            .join(path))
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
    _kernel_rev: Option<&str>,
    base_dir: &Path,
) -> Result<String, String> {
    if let Some(path) = kernel_path {
        let abs_kernel = fs::canonicalize(path).map_err(|e| format!("{e}: {}", path.display()))?;
        let rel = pathdiff(&abs_kernel, base_dir)?;
        Ok(rel.display().to_string())
    } else {
        Ok("../../kernel".to_string())
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

    let scarlet_manifest = format!(
        r#"schema_version = 1

[project]
name = "{name}"

[kernel]
package = "scarlet"
source = "{kernel_source}"
target = "{target}"
target_json = "{target_json_dir}"
"#
    );
    write_if_changed(&project_dir.join("scarlet.toml"), &scarlet_manifest)?;

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

    let modules_cargo_dir = project_dir.join(".scarlet/scarlet-modules/.cargo");
    fs::create_dir_all(&modules_cargo_dir)
        .map_err(|e| format!("failed to create {}: {e}", modules_cargo_dir.display()))?;
    let modules_cargo_config = render_cargo_config();
    fs::write(modules_cargo_dir.join("config.toml"), modules_cargo_config)
        .map_err(|e| format!("failed to write scarlet-modules .cargo/config.toml: {e}"))?;

    eprintln!("cargo-scarlet: created project '{name}'");
    eprintln!("cargo-scarlet: REQUIRED: update .cargo/config.toml with runner");
    eprintln!(
        "cargo-scarlet: REQUIRED: update .scarlet/scarlet-modules/.cargo/config.toml with target and build-std"
    );
    eprintln!("cargo-scarlet: REQUIRED: add linker script to lds/");
    eprintln!("cargo-scarlet: REQUIRED: implement boot entry in src/main.rs (arch_start_kernel)");

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
