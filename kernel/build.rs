// build.rs
use std::path::PathBuf;

fn main() {
    // Get the directory where build.rs is located
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    // Compile the ap_trampoline assembly file
    let trampoline_path = PathBuf::from(&dir).join("src/asm/ap_trampoline.s");

    // Compile with global NASM (or use cc with assembly)
    // Option 1: Using NASM (recommended for x86 assembly)
    if trampoline_path.exists() {
        println!("cargo:rerun-if-changed=src/asm/ap_trampoline.s");

        // Use cc crate to compile the assembly
        cc::Build::new()
            .file(trampoline_path)
            .flag("-nostdlib")
            .flag("-static")
            .flag("-fPIC")
            .flag("-m64")
            .compile("ap_trampoline");
    }
}
