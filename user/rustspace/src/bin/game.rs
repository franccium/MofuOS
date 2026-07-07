#![no_std]
#![no_main]

use core::arch::global_asm;

global_asm!(
    ".section .text.entry",
    ".global _start",
    "_start:",
    "    call rust_main",
    "    ud2",
);

const MSG_CREATED: &[u8] = b"game: window created, id=";
const MSG_NEWLINE: &[u8] = b"\n";
const MSG_FAILED: &[u8] = b"game: create_window failed\n";

#[unsafe(no_mangle)]
pub extern "C" fn rust_main() -> ! {
    unsafe {
        let window_id = rustspace::sys_create_window(320, 240, 100, 100);

        if window_id == u32::MAX {
            rustspace::sys_write(1, MSG_FAILED.as_ptr(), MSG_FAILED.len());
        } else {
            rustspace::sys_write(1, MSG_CREATED.as_ptr(), MSG_CREATED.len());
            write_u32(window_id);
            rustspace::sys_write(1, MSG_NEWLINE.as_ptr(), MSG_NEWLINE.len());
        }

        rustspace::sys_exit(0);
    }
}

unsafe fn write_u32(mut n: u32) {
    let mut buf = [0u8; 10];
    let mut pos = 10usize;

    if n == 0 {
        pos -= 1;
        buf[pos] = b'0';
    } else {
        while n > 0 {
            pos -= 1;
            buf[pos] = b'0' + (n % 10) as u8;
            n /= 10;
        }
    }

    unsafe { rustspace::sys_write(1, buf.as_ptr().add(pos), 10 - pos) };
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { rustspace::sys_exit(1) }
}
