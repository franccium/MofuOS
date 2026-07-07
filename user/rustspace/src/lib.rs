#![no_std]

pub const SYS_CREATE_PROCESS: u64 = 0;
pub const SYS_TERMINATE_PROCESS: u64 = 1;
pub const SYS_WRITE: u64 = 2;
pub const SYS_READ: u64 = 3;
pub const SYS_GET_LINE: u64 = 4;
pub const SYS_ALLOCATE: u64 = 5;
pub const SYS_CREATE_FILE: u64 = 6;
pub const SYS_REMOVE_FILE: u64 = 7;
pub const SYS_LOAD_FILE: u64 = 8;
pub const SYS_UNLOAD_FILE: u64 = 9;
pub const SYS_CREATE_WINDOW: u64 = 10;
pub const SYS_DESTROY_WINDOW: u64 = 11;
pub const SYS_YIELD: u64 = 998;
pub const SYS_EXIT: u64 = 999;

pub const SYS_ECHO: u64 = 997;

#[inline(always)]
pub unsafe fn syscall6(num: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> u64 {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "mov r10, rcx",
            "syscall",
            inout("rax") num => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("rcx") a4,
            in("r8") a5,
            in("r9") a6,
            lateout("rcx") _,
            lateout("r11") _,
            lateout("r10") _,
            options(nostack),
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall3(num: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    unsafe { syscall6(num, a1, a2, a3, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall1(num: u64, a1: u64) -> u64 {
    unsafe { syscall6(num, a1, 0, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn sys_write(fd: u64, buf: *const u8, count: usize) -> i64 {
    unsafe { syscall3(SYS_WRITE, fd, buf as u64, count as u64) as i64 }
}

#[inline(always)]
pub unsafe fn sys_exit(code: i32) -> ! {
    unsafe { syscall1(SYS_EXIT, code as u64) };
    loop {}
}

#[inline(always)]
pub unsafe fn sys_yield() {
    unsafe { syscall1(SYS_YIELD, 0) };
}

#[inline(always)]
pub unsafe fn sys_echo(val: u64) -> u64 {
    unsafe { syscall1(SYS_ECHO, val) }
}

#[inline(always)]
pub unsafe fn sys_create_window(width: u32, height: u32, x: i32, y: i32) -> u32 {
    unsafe {
        syscall6(
            SYS_CREATE_WINDOW,
            width as u64,
            height as u64,
            x as u64,
            y as u64,
            0,
            0,
        ) as u32
    }
}

#[inline(always)]
pub unsafe fn sys_destroy_window(window_id: u32) {
    unsafe { syscall1(SYS_DESTROY_WINDOW, window_id as u64) };
}
