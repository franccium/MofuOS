//use bootloader::BootConfig;
//use std::path::PathBuf;

fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    // Tell cargo to pass the linker script to the linker..
    println!("cargo:rustc-link-arg=-Tlinker-{arch}.ld");
    // ..and to re-run if it changes.
    println!("cargo:rerun-if-changed=linker-{arch}.ld");

    // let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    // let kernel_path = PathBuf::from(std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel").unwrap());

    // let uefi_path = out_dir.join("uefi.img");

    // let mut boot_config = BootConfig::default();
    // boot_config.frame_buffer.minimum_framebuffer_height = Some(400);
    // boot_config.frame_buffer.minimum_framebuffer_width = Some(600);

    // let mut uefi_boot = UefiBoot::new(&kernel_path);
    // uefi_boot.set_boot_config(&boot_config);
    // uefi_boot
    //     .create_disk_image(&uefi_path)
    //     .expect("failed to create disk image");

    // // let bios_path = out_dir.join("bios.img");
    // // bootloader::BiosBoot::new(&kernel)
    // //     .create_disk_image(&bios_path)
    // //     .unwrap();

    // println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());
    // //println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
}
