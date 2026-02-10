use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use flate2::write::GzEncoder;
use flate2::Compression;
use tar::Builder;

#[derive(Parser)]
#[command(name = "scpm-pack")]
#[command(about = "Package builder for Scarlet Package Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build {
        /// Path to the package directory
        package_dir: String,
        /// Output path for the package file
        #[arg(short, long)]
        output: Option<String>,
    },
    Init {
        /// Package name
        name: String,
        /// Package version
        #[arg(short, long, default_value = "0.1.0")]
        version: String,
        /// Package description
        #[arg(short = 'd', long)]
        description: Option<String>,
        /// Author
        #[arg(short = 'a', long)]
        author: Option<String>,
        /// Architecture (riscv64, aarch64, any)
        #[arg(short, long, default_value = "any")]
        arch: String,
        /// Create package directory
        #[arg(short, long)]
        dir: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build {
            package_dir,
            output,
        } => {
            if let Err(e) = build_package(&package_dir, output) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
        Commands::Init {
            name,
            version,
            description,
            author,
            arch,
            dir,
        } => {
            if let Err(e) = init_package(
                &name,
                &version,
                description.as_deref(),
                author.as_deref(),
                &arch,
                &dir,
            ) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
    }
}

fn build_package(package_dir: &str, output: Option<String>) -> Result<(), String> {
    let package_path = Path::new(package_dir);

    if !package_path.exists() {
        return Err(format!(
            "Package directory '{}' does not exist",
            package_dir
        ));
    }

    let metadata_file = package_path.join("package.toml");
    if !metadata_file.exists() {
        return Err(format!("package.toml not found in '{}'", package_dir));
    }

    let metadata: PackageMetadata = {
        let mut file = File::open(&metadata_file)
            .map_err(|e| format!("Failed to open package.toml: {}", e))?;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| format!("Failed to read package.toml: {}", e))?;
        toml::from_str(&content).map_err(|e| format!("Failed to parse package.toml: {}", e))?
    };

    let output_file =
        output.unwrap_or_else(|| format!("{}-{}.scarlet", metadata.name, metadata.version));

    println!("Building package: {}-{}", metadata.name, metadata.version);
    println!("Output: {}", output_file);

    let output_path = PathBuf::from(&output_file);
    let output_tar_gz =
        File::create(&output_path).map_err(|e| format!("Failed to create output file: {}", e))?;
    let encoder = GzEncoder::new(output_tar_gz, Compression::default());
    let mut tar = Builder::new(encoder);

    tar.append_dir_all(".", package_path)
        .map_err(|e| format!("Failed to add files to archive: {}", e))?;

    let encoder = tar
        .into_inner()
        .map_err(|e| format!("Failed to finalize archive: {}", e))?;
    encoder
        .finish()
        .map_err(|e| format!("Failed to compress archive: {}", e))?;

    println!("Package built successfully!");

    Ok(())
}

fn init_package(
    name: &str,
    version: &str,
    description: Option<&str>,
    author: Option<&str>,
    arch: &str,
    dir: &str,
) -> Result<(), String> {
    let package_dir = Path::new(dir);
    if package_dir.exists() {
        return Err(format!("Directory '{}' already exists", dir));
    }

    fs::create_dir_all(package_dir.join("bin"))
        .map_err(|e| format!("Failed to create bin directory: {}", e))?;
    fs::create_dir_all(package_dir.join("lib"))
        .map_err(|e| format!("Failed to create lib directory: {}", e))?;

    let mut metadata = PackageMetadata {
        name: name.to_string(),
        version: version.to_string(),
        description: description.unwrap_or("").to_string(),
        author: author.map(String::from),
        homepage: None,
        binaries: Vec::new(),
        libraries: Vec::new(),
        dependencies: Vec::new(),
        architecture: arch.to_string(),
        license: None,
    };

    // Parse files section for dpkg-style file list
    for line in toml_content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');

            if key == "files" {
                // Parse files list: "path1", "path2", ...
                for file_path in value.split(',') {
                    let file_path = file_path.trim();
                    if !file_path.is_empty() {
                        // Store as simple string for now (could be improved to struct)
                        metadata
                            .installed_files
                            .push(format!("/{}", file_path.trim()));
                    }
                }
            } else {
                // Preserve existing sections
                match (current_section.as_deref(), key) {
                    (Some("package"), "name") => metadata.name = value.to_string(),
                    (Some("package"), "version") => metadata.version = value.to_string(),
                    (Some("package"), "description") => metadata.description = value.to_string(),
                    (Some("package"), "author") => metadata.author = Some(value.to_string()),
                    (Some("bin"), "name") => {
                        if metadata.bin_name.is_empty() {
                            metadata.bin_name = value.to_string();
                        } else {
                            metadata.binaries.push(value.to_string());
                        }
                    }
                    (Some("package"), "binaries") => metadata.binaries.push(value.to_string()),
                    _ => {}
                }
            }
        }
    }

    let toml_content = toml::to_string_pretty(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
    let mut file = File::create(&metadata_path)
        .map_err(|e| format!("Failed to create package.toml: {}", e))?;
    file.write_all(toml_content.as_bytes())
        .map_err(|e| format!("Failed to write to package.toml: {}", e))?;

    println!("Initialized package: {}", name);
    println!("  Directory: {}", dir);
    println!("  Metadata: package.toml");
    println!("  Binaries dir: bin/");
    println!("  Libraries dir: lib/");

    let toml_content = toml::to_string_pretty(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;

    let metadata_path = package_dir.join("package.toml");
    let mut file = File::create(&metadata_path)
        .map_err(|e| format!("Failed to create package.toml: {}", e))?;
    file.write_all(toml_content.as_bytes())
        .map_err(|e| format!("Failed to write package.toml: {}", e))?;

    println!("Initialized package: {}", name);
    println!("  Directory: {}", dir);
    println!("  Metadata: package.toml");
    println!("  Binaries dir: bin/");
    println!("  Libraries dir: lib/");

    Ok(())
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub binaries: Vec<String>,
    pub libraries: Vec<String>,
    pub dependencies: Vec<Dependency>,
    pub architecture: String,
    pub license: Option<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Dependency {
    pub name: String,
    pub version: Option<String>,
}
