fn main() {
    println!("cargo:rerun-if-changed=ui/slint_demo.slint");
    println!("cargo:rerun-if-changed=build.rs");

    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);

    slint_build::compile_with_config("ui/slint_demo.slint", config)
        .expect("Failed to compile Slint UI");
}
