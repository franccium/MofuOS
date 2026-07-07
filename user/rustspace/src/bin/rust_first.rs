#![no_std]
#![no_main]

use core::arch::global_asm;

global_asm!(
    ".section .text.entry",
    ".global _start",
    "_start:",
    "    call rust_main",
    // rust_main is -> !, so this is unreachable
    "    ud2",
);

const MSG: &[u8] = b"the userspace is in rust btw";

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    unsafe {
        let syscall_result = rustspace::sys_echo(123);

        rustspace::sys_write(1, MSG.as_ptr(), MSG.len());

        let hi = b'0' + ((syscall_result / 10) % 10) as u8;
        let lo = b'0' + (syscall_result % 10) as u8;
        let result_msg = [
            b'e', b'c', b'h', b'o', b'(', b'1', b'2', b'3', b')', b'=', b' ', hi, lo, b'\n',
        ];
        rustspace::sys_write(1, result_msg.as_ptr(), result_msg.len());

        rustspace::sys_exit(0);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { rustspace::sys_exit(1) }
}
