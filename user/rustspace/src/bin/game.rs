#![no_std]
#![no_main]

extern crate alloc;
extern crate rustspace;

use alloc::format;
use core::arch::global_asm;

global_asm!(
    ".section .text.entry",
    ".global _start",
    "_start:",
    "    call rust_main",
    "    ud2",
);

#[unsafe(no_mangle)]
pub extern "C" fn rust_main() -> ! {
    let window_id = unsafe { rustspace::sys_create_window(320, 240, 100, 100) };

    if window_id == u32::MAX {
        rustspace::println!("game: create_window failed");
    } else {
        let msg = format!("game: window created, id={}", window_id);
        rustspace::println!("{}", msg);
        rustspace::println!("formatted float: {:.9}", 1.61803398875f64);
    }

    unsafe { rustspace::sys_exit(0) }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    rustspace::println!("game: panic");
    unsafe { rustspace::sys_exit(1) }
}
