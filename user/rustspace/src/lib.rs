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
pub const SYS_LOAD_FILE: u64 = 8;
pub const SYS_UNLOAD_FILE: u64 = 9;
pub const SYS_CREATE_WINDOW: u64 = 10;
pub const SYS_DESTROY_WINDOW: u64 = 11;
pub const SYS_MAP_WINDOW_BUFFER: u64 = 12;
pub const SYS_PRESENT_WINDOW: u64 = 13;
pub const SYS_GET_WINDOW_SIZE: u64 = 14;
pub const SYS_FOCUS_WINDOW: u64 = 15;

// Filesystem syscalls
pub const SYS_OPEN_FILE: u64 = 20;
pub const SYS_CLOSE_FILE: u64 = 21;
pub const SYS_READ_FILE: u64 = 22;
pub const SYS_WRITE_FILE: u64 = 23;
pub const SYS_STAT_FILE: u64 = 24;
pub const SYS_LIST_DIR: u64 = 25;
pub const SYS_CREATE_FILE: u64 = 26;
pub const SYS_CREATE_DIR: u64 = 27;
pub const SYS_DELETE: u64 = 28;

pub const SYS_GET_CPU_INFO: u64 = 970;

pub const SYS_YIELD: u64 = 998;
pub const SYS_EXIT: u64 = 999;
pub const SYS_ECHO: u64 = 997;

pub const FD_FLAG_READ: u8 = 0x01;
pub const FD_FLAG_WRITE: u8 = 0x02;

pub const FS_NAME_LEN: usize = 16;

/// Flat directory entry returned by sys_list_dir.
/// Must match the kernel-side DirEntryFlat layout exactly.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DirEntryFlat {
    pub name: [u8; FS_NAME_LEN],
    pub name_len: u8,
    pub is_dir: u8,
    pub _pad: [u8; 6],
    pub size: u64,
    pub created_time: u32,
    pub modified_time: u32,
}

impl DirEntryFlat {
    pub const fn zeroed() -> Self {
        Self {
            name: [0u8; FS_NAME_LEN],
            name_len: 0,
            is_dir: 0,
            _pad: [0u8; 6],
            size: 0,
            created_time: 0,
            modified_time: 0,
        }
    }

    pub fn name_str(&self) -> &str {
        let len = self.name_len as usize;
        core::str::from_utf8(&self.name[..len]).unwrap_or("")
    }
}

/// Flat stat result returned by sys_stat.
/// Must match the kernel-side StatFlat layout exactly.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StatFlat {
    pub name: [u8; FS_NAME_LEN],
    pub name_len: u8,
    pub is_dir: u8,
    pub size: u64,
    pub created_time: u32,
    pub modified_time: u32,
}

impl StatFlat {
    pub const fn zeroed() -> Self {
        Self {
            name: [0u8; FS_NAME_LEN],
            name_len: 0,
            is_dir: 0,
            size: 0,
            created_time: 0,
            modified_time: 0,
        }
    }

    pub fn name_str(&self) -> &str {
        let len = self.name_len as usize;
        core::str::from_utf8(&self.name[..len]).unwrap_or("")
    }
}

#[repr(C, align(16))]
pub struct CpuInfoFlat {
    pub vendor: u8,
    pub family: u8,
    pub model: u8,
    pub stepping: u8,
    pub display_family: u16,
    pub display_model: u8,
    pub cache_line_size: u8,
    pub apic_id: u8,
    pub features: u32,
    pub tsc_frequency_hz: u64,
    pub boot_tsc: u64,
    pub max_cpuid_leaf: u32,
    pub max_extended_cpuid_leaf: u32,
    pub core_count: u8,
}

impl CpuInfoFlat {
    pub const fn zeroed() -> Self {
        Self {
            vendor: 0,
            family: 0,
            model: 0,
            stepping: 0,
            display_family: 0,
            display_model: 0,
            cache_line_size: 0,
            apic_id: 0,
            features: 0,
            tsc_frequency_hz: 0,
            boot_tsc: 0,
            max_cpuid_leaf: 0,
            max_extended_cpuid_leaf: 0,
            core_count: 0,
        }
    }
}

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
pub unsafe fn syscall1(num: u64, a1: u64) -> u64 {
    unsafe { syscall6(num, a1, 0, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall2(num: u64, a1: u64, a2: u64) -> u64 {
    unsafe { syscall6(num, a1, a2, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall3(num: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    unsafe { syscall6(num, a1, a2, a3, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall4(num: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    unsafe { syscall6(num, a1, a2, a3, a4, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall5(num: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> u64 {
    unsafe { syscall6(num, a1, a2, a3, a4, a5, 0) }
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

// --- Filesystem wrappers ---

/// Open a file or directory by path.
/// flags: FD_FLAG_READ | FD_FLAG_WRITE
/// Returns a file descriptor index, or usize::MAX on failure.
#[inline(always)]
pub unsafe fn sys_open_file(path: &str, flags: u8) -> usize {
    let ret = unsafe {
        syscall3(
            SYS_OPEN_FILE,
            path.as_ptr() as u64,
            path.len() as u64,
            flags as u64,
        )
    };
    if ret == u64::MAX {
        usize::MAX
    } else {
        ret as usize
    }
}

/// Close a file descriptor.
/// Returns true on success.
#[inline(always)]
pub unsafe fn sys_close_file(fd: usize) -> bool {
    unsafe { syscall1(SYS_CLOSE_FILE, fd as u64) != u64::MAX }
}

/// Read up to buf.len() bytes from an open fd
/// Returns the number of bytes read, or usize::MAX on failure.
#[inline(always)]
pub unsafe fn sys_read_file(fd: usize, buf: &mut [u8]) -> usize {
    let ret = unsafe {
        syscall4(
            SYS_READ_FILE,
            fd as u64,
            0,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret != u64::MAX {
        ret as usize
    } else {
        usize::MAX
    }
}

/// Read up to buf.len() bytes from an open fd at the current offset.
/// Returns the number of bytes read, or usize::MAX on failure.
#[inline(always)]
pub unsafe fn sys_read_file_at(fd: usize, offset: usize, buf: &mut [u8]) -> usize {
    let ret = unsafe {
        syscall4(
            SYS_READ_FILE,
            fd as u64,
            offset as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret != u64::MAX {
        ret as usize
    } else {
        usize::MAX
    }
}

/// Write buf to an open fd at the current offset.
/// Returns the number of bytes written, or usize::MAX on failure.
#[inline(always)]
pub unsafe fn sys_write_file(fd: usize, offset: usize, buf: &[u8]) -> usize {
    let ret = unsafe {
        syscall4(
            SYS_WRITE_FILE,
            fd as u64,
            offset as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret == u64::MAX {
        usize::MAX
    } else {
        ret as usize
    }
}

/// Stat a path. Fills out_stat on success. Returns true on success.
#[inline(always)]
pub unsafe fn sys_stat_file(path: &str, out_stat: &mut StatFlat) -> bool {
    let ret = unsafe {
        syscall3(
            SYS_STAT_FILE,
            path.as_ptr() as u64,
            path.len() as u64,
            out_stat as *mut StatFlat as u64,
        )
    };
    ret != u64::MAX
}

/// List the contents of a directory into out_entries.
/// Returns the number of entries written, or usize::MAX on failure.
#[inline(always)]
pub unsafe fn sys_list_dir(path: &str, out_entries: &mut [DirEntryFlat]) -> usize {
    let byte_len = out_entries.len() * core::mem::size_of::<DirEntryFlat>();
    let ret = unsafe {
        syscall6(
            SYS_LIST_DIR,
            path.as_ptr() as u64,
            path.len() as u64,
            out_entries.as_mut_ptr() as u64,
            byte_len as u64,
            0,
            0,
        )
    };
    if ret == u64::MAX {
        usize::MAX
    } else {
        ret as usize
    }
}

/// Create a new file at the given path. Returns true on success.
#[inline(always)]
pub unsafe fn sys_create_file(path: &str) -> bool {
    let ret = unsafe { syscall3(SYS_CREATE_FILE, path.as_ptr() as u64, path.len() as u64, 0) };
    ret != u64::MAX
}

/// Create a new directory at the given path. Returns true on success.
#[inline(always)]
pub unsafe fn sys_create_dir(path: &str) -> bool {
    let ret = unsafe { syscall3(SYS_CREATE_DIR, path.as_ptr() as u64, path.len() as u64, 0) };
    ret != u64::MAX
}

/// Delete a file or directory at the given path. Returns true on success.
#[inline(always)]
pub unsafe fn sys_delete(path: &str) -> bool {
    let ret = unsafe { syscall3(SYS_DELETE, path.as_ptr() as u64, path.len() as u64, 0) };
    ret != u64::MAX
}

#[inline(always)]
pub unsafe fn sys_get_cpu_info(out_info: &mut CpuInfoFlat) -> bool {
    let ret = unsafe {
        syscall2(
            SYS_GET_CPU_INFO,
            out_info as *mut CpuInfoFlat as u64,
            core::mem::size_of::<CpuInfoFlat>() as u64,
        )
    };
    ret == 0
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

    pub fn preallocate(&self, prealloc_size: usize) {
        let base = unsafe { sys_allocate(prealloc_size) };
        if base != INVALID_ALLOC {
            self.cursor
                .store(base as usize, core::sync::atomic::Ordering::Release);
            self.end.store(
                base as usize + prealloc_size,
                core::sync::atomic::Ordering::Release,
            );
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

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct InputEvent {
    pub event_type: EventType,
    pub _pad: [u8; 3],
    pub value: u32,
    pub extra: u32,
    pub reserved: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EventType {
    None = 0,
    KeyEvent = 1,
    MouseEvent = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyState {
    Pressed = 0,
    Released = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Keys {
    ArrowUp = 0x110000,
    ArrowDown = 0x110001,
    ArrowLeft = 0x110002,
    ArrowRight = 0x110003,
    LeftAlt = 0x120000,
    Backspace = 0x080000,
    Tab = 0x090000,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MouseButtons: u16 {
        const LEFT = 0b0000_0001;
        const RIGHT = 0b0000_0010;
        const MIDDLE = 0b0000_0100;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub x_delta: i16,
    pub y_delta: i16,
    pub buttons: MouseButtons,
    pub x_overflow: bool,
    pub y_overflow: bool,
}

pub struct AsciiChar;
impl AsciiChar {
    pub const BACKSPACE: char = '\x08';
    pub const TAB: char = '\x09';
    pub const NEWLINE: char = '\n';
    pub const CARRIAGE_RETURN: char = '\r';
    pub const ESCAPE: char = '\x1B';
    pub const DELETE: char = '\x7F';
    pub const SPACE: char = ' ';
}

const PAGE_SIZE: usize = 4096;
const BUFFER_HEADER_SIZE: usize = core::mem::size_of::<AtomicU32>() * 3;
const MAX_EVENT_COUNT: usize =
    (PAGE_SIZE - BUFFER_HEADER_SIZE) / core::mem::size_of::<InputEvent>();

#[repr(C, align(4096))]
pub struct EventBuffer {
    pub write_idx: AtomicU32,
    pub read_idx: AtomicU32,
    pub event_count: AtomicU32,
    pub events: [InputEvent; MAX_EVENT_COUNT],
}

pub struct EventReader {
    buffer: &'static EventBuffer,
}

pub const EVENT_BUFFER_ADDR: usize = 0x0000_0007_0000_0000;

impl EventReader {
    /// Create a new event reader for the buffer at the given address
    /// The address must point to a valid, kernel-mapped EventBuffer
    pub unsafe fn new(buffer_addr: usize) -> Self {
        let buffer = unsafe { &*(buffer_addr as *const EventBuffer) };
        Self { buffer }
    }

    pub fn has_events(&self) -> bool {
        self.buffer.event_count.load(Ordering::Acquire) > 0
    }

    pub fn try_read(&mut self) -> Option<InputEvent> {
        if self.buffer.event_count.load(Ordering::Acquire) == 0 {
            return None;
        }

        let idx = self.buffer.read_idx.load(Ordering::Acquire) as usize;

        let event =
            unsafe { core::ptr::read_volatile(&self.buffer.events[idx] as *const InputEvent) };

        core::sync::atomic::fence(Ordering::Acquire);

        let new_idx = ((idx + 1) % MAX_EVENT_COUNT) as u32;
        self.buffer.read_idx.store(new_idx, Ordering::Release);

        self.buffer.event_count.fetch_sub(1, Ordering::Release);

        Some(event)
    }

    pub fn read_blocking(&mut self) -> InputEvent {
        loop {
            if let Some(event) = self.try_read() {
                return event;
            }
            unsafe { sys_yield() }
        }
    }

    pub fn decode_mouse(&self) -> MouseEvent {
        let x_delta = self.value as i32 as i16;
        let y_delta = (self.extra & 0xFFFF) as i16 as i16;
        let buttons = MouseButtons::from_bits_truncate(((self.extra >> 16) as u16) & 0x07);

        let overflow_bits = (self.extra >> 19) & 0x03;
        let x_overflow = (overflow_bits & 0x01) != 0;
        let y_overflow = (overflow_bits & 0x02) != 0;

        MouseEvent {
            x_delta,
            y_delta,
            buttons,
            x_overflow,
            y_overflow,
        }
    }
}

pub unsafe fn tsc_read() -> (u64, u32) {
    let low: u32;
    let high: u32;
    let core: u32;

    unsafe {
        asm!(
            "lfence",
            "rdtscp",
            out("eax") low,
            out("edx") high,
            out("ecx") core,
            options(nostack, preserves_flags)
        );
    }

    let tsc = ((high as u64) << 32) | (low as u64);
    (tsc, core)
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

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CacheStatsFlat {
    pub total_files: u64,
    pub total_bytes: u64,
    pub max_bytes: u64,
    pub dirty_files: u64,
}

impl CacheStatsFlat {
    pub const fn zeroed() -> Self {
        Self {
            total_files: 0,
            total_bytes: 0,
            max_bytes: 0,
            dirty_files: 0,
        }
    }
}

// Cache importance enum
#[repr(u8)]
pub enum CacheImportance {
    Minimal = 0,
    Low = 1,
    Normal = 2,
    High = 3,
    VeryHigh = 4,
    Critical = 5,
    Resident = 6,
}

// Cache syscall wrappers
pub unsafe fn sys_pin_file(path: &str) -> u64 {
    let path_bytes = path.as_bytes();
    syscall4(
        30,
        path_bytes.as_ptr() as u64,
        path_bytes.len() as u64,
        0,
        0,
    )
}

pub unsafe fn sys_unpin_file(path: &str) -> u64 {
    let path_bytes = path.as_bytes();
    syscall4(
        31,
        path_bytes.as_ptr() as u64,
        path_bytes.len() as u64,
        0,
        0,
    )
}

pub unsafe fn sys_reserve_cache(path: &str, importance: u8) -> u64 {
    let path_bytes = path.as_bytes();
    syscall4(
        32,
        path_bytes.as_ptr() as u64,
        path_bytes.len() as u64,
        importance as u64,
        0,
    )
}

pub unsafe fn sys_evict_directory(path: &str) -> u64 {
    let path_bytes = path.as_bytes();
    syscall4(
        33,
        path_bytes.as_ptr() as u64,
        path_bytes.len() as u64,
        0,
        0,
    )
}

pub unsafe fn sys_get_cache_stats(stats: &mut CacheStatsFlat) -> bool {
    let ret = syscall4(34, stats as *mut CacheStatsFlat as u64, 0, 0, 0);
    ret == 0
}

/// Flush all dirty cached files belonging to the calling process's open fds.
/// Returns true on success.
pub unsafe fn sys_flush_file_cache() -> bool {
    syscall1(35, 0) == 0
}
