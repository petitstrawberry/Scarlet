use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let source_path = PathBuf::from("shaders/cube.wgsl");
    println!("cargo:rerun-if-changed={}", source_path.display());
    let source = fs::read_to_string(&source_path)?;
    let module = naga::front::wgsl::parse_str(&source)?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)?;
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unavailable")?);

    compile_shader(
        &module,
        &info,
        naga::ShaderStage::Vertex,
        "vs_main",
        output.join("cube.vert.spv"),
    )?;
    compile_shader(
        &module,
        &info,
        naga::ShaderStage::Fragment,
        "fs_main",
        output.join("cube.frag.spv"),
    )?;
    Ok(())
}

fn compile_shader(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    stage: naga::ShaderStage,
    entry_point: &str,
    output: PathBuf,
) -> Result<(), Box<dyn Error>> {
    let pipeline = naga::back::spv::PipelineOptions {
        shader_stage: stage,
        entry_point: entry_point.into(),
    };
    let words = naga::back::spv::write_vec(
        module,
        info,
        &naga::back::spv::Options::default(),
        Some(&pipeline),
    )?;
    let bytes = words
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    fs::write(output, bytes)?;
    Ok(())
}
