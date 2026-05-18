// build.rs
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Get the directory where build.rs is located
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();

    // Prefer the capital-"S" assembly file which is processed by the C preprocessor.
    let trampoline_src = PathBuf::from(&dir).join("src/asm/ap_trampoline.S");
    let linker_script = PathBuf::from(&dir).join("ap_trampoline.ld");

    println!("cargo:rerun-if-changed=src/asm/ap_trampoline.S");
    println!("cargo:rerun-if-changed=ap_trampoline.ld");

    if !trampoline_src.exists() {
        eprintln!("AP trampoline source not found: {:?}", trampoline_src);
        return;
    }

    if !linker_script.exists() {
        eprintln!("AP trampoline linker script not found: {:?}", linker_script);
        return;
    }

    let out_path = PathBuf::from(&out_dir);

    // Step 1: Assemble AP trampoline with GCC/Clang assembler
    let obj_file = out_path.join("ap_trampoline.o");
    let status = Command::new("cc")
        .arg("-c")
        .arg("-nostdlib")
        .arg("-no-pie")
        .arg("-m64")
        .arg("-x")
        .arg("assembler-with-cpp")
        .arg(&trampoline_src)
        .arg("-o")
        .arg(&obj_file)
        .status()
        .expect("Failed to assemble AP trampoline");

    if !status.success() {
        panic!("AP trampoline assembly failed");
    }

    // Step 2: Link AP trampoline with custom linker script
    let elf_file = out_path.join("ap_trampoline.elf");
    let status = Command::new("ld")
        .arg("-T")
        .arg(&linker_script)
        .arg(&obj_file)
        .arg("-o")
        .arg(&elf_file)
        .status()
        .expect("Failed to link AP trampoline");

    if !status.success() {
        panic!("AP trampoline linking failed");
    }

    // Step 3: Extract binary blob from ELF
    let bin_file = out_path.join("ap_trampoline.bin");
    let status = Command::new("objcopy")
        .arg("-O")
        .arg("binary")
        .arg(&elf_file)
        .arg(&bin_file)
        .status()
        .expect("Failed to extract AP trampoline binary");

    if !status.success() {
        panic!("AP trampoline binary extraction failed");
    }

    // Step 4: Verify the binary was created and print size
    if let Ok(metadata) = std::fs::metadata(&bin_file) {
        let size = metadata.len();
        println!("cargo:warning=AP trampoline binary size: {} bytes", size);
    }

    println!("cargo:rustc-env=AP_TRAMPOLINE_BIN={}", bin_file.display());
}
