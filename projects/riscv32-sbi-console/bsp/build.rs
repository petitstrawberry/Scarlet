fn main() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../kernel/lds/riscv32_sbi.ld");
    println!("cargo:rustc-link-arg=-T{}", script.display());
    println!("cargo:rerun-if-changed={}", script.display());
}
