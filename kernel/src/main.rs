#![no_std]
#![no_main]

mod boot;

use core::arch::asm;
use kernel::{serial_print, serial_println};
use x86_64::instructions::hlt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) -> ! {
    use x86_64::instructions::{nop, port::Port};

    unsafe {
        let mut port = Port::new(0xF4);
        port.write(exit_code as u32);
    }

    loop {
        nop();
    }
}

#[panic_handler]
fn rust_panic(info: &core::panic::PanicInfo) -> ! {
    serial_println!("PANIC: {:#?}", info);
    exit_qemu(QemuExitCode::Failed);
}

fn main() -> ! {
    serial_println!("Welcome to MofuOS!");

    serial_println!("Boot info: {:#?}", boot::boot_info().stack_size);

    loop {
        hlt();
    }

    exit_qemu(QemuExitCode::Success);
}
