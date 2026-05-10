use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::env;
use std::process::{Command, exit};

fn main() {
    // let uefi_path = env!("UEFI_PATH");
    // //let bios_path = env!("BIOS_PATH");

    // let args: Vec<String> = env::args().collect();
    // let prog = &args[0];

    // let mut cmd = Command::new("qemu-system-x86_64");
    // cmd.arg("-serial").arg("mon:stdio");
    // cmd.arg("-display").arg("none");
    // cmd.arg("-device").arg("isa-debug-exit,iobase=0xf4,iosize=0x04");

    // let ovmf = Prebuilt::fetch(Source::LATEST, "target/ovmf").expect("Failed to update prebuilt OVMF");
    // let code = ovmf.get_file(Arch::x64, FileType::Code);
    // let vars = ovmf.get_file(Arch::x64, FileType::Vars);

    // cmd.arg("-drive").arg(format!("format=raw, file={uefi_path}"));
    // cmd.arg("-drive").arg(format!("if=pflash,format=raw,unit=0,file={},readonly=on", code.display()));
    // cmd.arg("-drive").arg(format!("if=pflash,format=raw,unit=1,file={},snapshot=on", vars.display()));

    // let mut child = cmd.spawn().expect("Failed to launch QEMU");
    // let exit_status = child.wait().expect("Failed to wait on QEMU");
    // match status.code().unwrap_or(1) {
    //     0x10 => 0,
    //     0x11 => 1,
    //     _ => 2,
    // };
}