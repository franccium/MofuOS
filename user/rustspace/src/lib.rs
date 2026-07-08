#![no_std]
#![feature(alloc_error_handler)]
#![feature(portable_simd)]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};

pub mod gfx;

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
pub const SYS_MAP_WINDOW_BUFFER: u64 = 12;
pub const SYS_PRESENT_WINDOW: u64 = 13;
pub const SYS_GET_WINDOW_SIZE: u64 = 14;
pub const SYS_YIELD: u64 = 998;
pub const SYS_EXIT: u64 = 999;
pub const SYS_ECHO: u64 = 997;

pub const INVALID_ALLOC: u64 = u64::MAX;

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
            in("r8")  a5,
            in("r9")  a6,
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

/// Returns the base address of the allocated region, or u64::MAX on failure
#[inline(always)]
pub unsafe fn sys_allocate(size: usize) -> u64 {
    unsafe { syscall1(SYS_ALLOCATE, size as u64) }
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

/// Map the back buffer of a kernel window into this process's address space.
/// Returns a pointer to the first pixel (XRGB8888 u32 array), or null on failure.
#[inline(always)]
pub unsafe fn sys_map_window_buffer(window_id: u32) -> *mut u32 {
    let addr = unsafe { syscall1(SYS_MAP_WINDOW_BUFFER, window_id as u64) };
    if addr == u64::MAX {
        core::ptr::null_mut()
    } else {
        addr as *mut u32
    }
}

/// Signal the kernel that the back buffer is ready to be presented.
#[inline(always)]
pub unsafe fn sys_present_window(window_id: u32) {
    unsafe { syscall1(SYS_PRESENT_WINDOW, window_id as u64) };
}

/// Returns (width, height) of the window, or (0, 0) on failure.
#[inline(always)]
pub unsafe fn sys_get_window_size(window_id: u32) -> (u32, u32) {
    let packed = unsafe { syscall1(SYS_GET_WINDOW_SIZE, window_id as u64) };
    if packed == u64::MAX {
        (0, 0)
    } else {
        ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32)
    }
}

// Arena bump allocator
// Asks the kernel for SLAB_SIZE bytes at a time and hands out addresses
// from within that arena. Only calls sys_allocate again when the current arena is exhausted
const SLAB_SIZE: usize = 64 * 1024;

pub struct Arena {
    cursor: core::sync::atomic::AtomicUsize,
    end: core::sync::atomic::AtomicUsize,
}

impl Arena {
    const fn new() -> Self {
        Self {
            cursor: core::sync::atomic::AtomicUsize::new(0),
            end: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    unsafe fn grow(&self) -> bool {
        let base = unsafe { sys_allocate(SLAB_SIZE) };
        if base != INVALID_ALLOC {
            self.cursor
                .store(base as usize, core::sync::atomic::Ordering::Release);
            self.end.store(
                base as usize + SLAB_SIZE,
                core::sync::atomic::Ordering::Release,
            );
            return true;
        }
        false
    }
}

#[global_allocator]
pub static ALLOCATOR: Arena = Arena::new();

unsafe impl GlobalAlloc for Arena {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        use core::sync::atomic::Ordering::{AcqRel, Acquire};

        loop {
            let cursor = self.cursor.load(Acquire);
            let end = self.end.load(Acquire);

            let aligned = (cursor + layout.align() - 1) & !(layout.align() - 1);
            let next = aligned + layout.size();

            if next <= end {
                match self.cursor.compare_exchange(cursor, next, AcqRel, Acquire) {
                    Ok(_) => return aligned as *mut u8,
                    Err(_) => continue,
                }
            }

            if !unsafe { self.grow() } {
                return core::ptr::null_mut();
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[alloc_error_handler]
fn alloc_error(_layout: Layout) -> ! {
    unsafe { sys_write(2, b"alloc error\n".as_ptr(), 12) };
    unsafe { sys_exit(1) }
}

pub struct Serial;

impl core::fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        unsafe { sys_write(1, s.as_ptr(), s.len()) };
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = core::fmt::write(&mut $crate::Serial, core::format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = core::fmt::write(&mut $crate::Serial, core::format_args!($($arg)*));
        unsafe { $crate::sys_write(1, b"\n".as_ptr(), 1) };
    }};
}
