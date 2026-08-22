use crate::data_structures::circular_buffer::{CircularBuffer, CircularBufferInfo};
use crate::events::event_buffer::EventBuffer;
use crate::filesystem::sirius::{DirEntryFlat, FS_NAME_LEN, FilesystemDriver, StatFlat};
use crate::graphics::window::WindowInfo;
use crate::interrupts::{BOOT_TSC, TSC_FREQUENCY_HZ};
use crate::io::serial;
use crate::memory::get_frame_allocator;
use crate::memory::usermem::USER_MEM_MAX_ADDRESS;
use crate::process::core_pool::TOTAL_CORE_COUNT;
use crate::process::elf_loader::ODYS_ELF;
use crate::process::process::FileDescriptor;
use crate::process::{CORE_POOL, ElfLoadInfo};
use crate::serial_println;
use crate::util::cpuinfo::{CpuInfoFlat, get_cpu_info_for_core};
use crate::util::msr::msr_write;
use crate::{
    process::{
        process::INVALID_PID,
        process_manager::{ARCHE_PID, PROCESS_MANAGER},
        scheduler,
    },
    serial_println_core,
    util::cpuinfo::get_current_core_id,
};
use alloc::string::String;
use alloc::vec::Vec;
use x86_64::structures::paging::{Mapper, PageTableFlags};
use core::arch::naked_asm;
use core::sync::atomic::Ordering;
use x86_64::registers::model_specific::{Efer, EferFlags};

#[cfg(feature = "use_cached_fs")]
use crate::filesystem::file_cache::CacheImportance;

const SYSCALL_STACK_SIZE: usize = 4096 * 16; // 64 KiB per core

#[inline]
fn validate_user_ptr(ptr: usize, len: usize) -> bool {
    let end = ptr.saturating_add(len);
    ptr != 0 && end <= USER_MEM_MAX_ADDRESS && end >= ptr
}

fn read_user_string(ptr: usize, len: usize) -> Option<String> {
    if !validate_user_ptr(ptr, len) {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len) };
    match core::str::from_utf8(bytes) {
        Ok(s) => Some(String::from(s)),
        Err(_) => None,
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallError {
    Success = 0,
    InvalidPtr = 1,
    PermissionDenied = 2,
    OutOfMemory = 3,
    ProcessNotFound = 4,
    InvalidFd = 5,
    SyscallNotFound = 999,
}

#[repr(u64)]
pub enum SyscallNumber {
    CreateProcess = 0,
    TerminateProcess = 1,
    Write = 2,
    Read = 3,
    GetLine = 4,
    Allocate = 5,
    LoadFile = 8,
    UnloadFile = 9,
    CreateWindow = 10,
    DestroyWindow = 11,
    MapWindowBuffer = 12,
    PresentWindow = 13,
    GetWindowSize = 14,
    FocusWindow = 15,
    GetWindowInfo = 16,
    // Filesystem syscalls
    OpenFile = 20,
    CloseFile = 21,
    ReadFile = 22,
    WriteFile = 23,
    StatFile = 24,
    ListDir = 25,
    CreateFile = 26,
    CreateDir = 27,
    Delete = 28,
    // Cache control syscalls (only meaningful with use_cached_fs feature)
    PinFile = 30,
    UnpinFile = 31,
    ReserveCache = 32,
    EvictDirectory = 33,
    GetCacheStats = 34,
    FlushFileCache = 35,

    CreateCircularBuffer = 600,

    GetCpuInfo = 970,
    GetProcessInfo = 996,
    GetPID = 997,
    Yield = 998,
    Exit = 999,
}

#[repr(C, align(64))]
struct PerCoreSyscallData {
    stack_top: u64,
    _stack: [u8; SYSCALL_STACK_SIZE],
}

impl PerCoreSyscallData {
    const fn zeroed() -> Self {
        Self {
            stack_top: 0,
            _stack: [0u8; SYSCALL_STACK_SIZE],
        }
    }
}

#[repr(C, align(4096))]
struct GuardedSyscallSlot {
    guard: [u8; 4096],
    data: PerCoreSyscallData,
}

impl GuardedSyscallSlot {
    const fn new() -> Self {
        Self {
            guard: [0u8; 4096],
            data: PerCoreSyscallData::zeroed(),
        }
    }
}

static mut SYSCALL_SLOTS: [GuardedSyscallSlot; crate::MAX_CORES as usize] = {
    const EMPTY: GuardedSyscallSlot = GuardedSyscallSlot::new();
    [EMPTY; crate::MAX_CORES as usize]
};



pub fn syscall_guard_page(core_id: u8) -> x86_64::VirtAddr {
    unsafe {
        let slot = &SYSCALL_SLOTS[core_id as usize];
        x86_64::VirtAddr::new(slot.guard.as_ptr() as u64)
    }
}

pub fn syscall_bounds(core_id: u8) -> (x86_64::VirtAddr, x86_64::VirtAddr) {
    unsafe {
        let slot = &SYSCALL_SLOTS[core_id as usize];
        let bottom = x86_64::VirtAddr::new(slot.data._stack.as_ptr() as u64);
        let top = x86_64::VirtAddr::new(slot.data._stack.as_ptr().add(SYSCALL_STACK_SIZE) as u64);
        (bottom, top)
    }
}

pub fn syscall_stack_top(core_id: u8) -> u64 {
    unsafe {
        let slot = &SYSCALL_SLOTS[core_id as usize];
        slot.data._stack.as_ptr().add(SYSCALL_STACK_SIZE) as u64
    }
}

pub fn install_syscall_guard_pages(frame_allocator: &mut crate::memory::memory::MemoryMapFrameAllocator) {
    for core_id in 0..crate::MAX_CORES as usize {
        unsafe {
            crate::stack_guard::ensure_guard_unmapped(syscall_guard_page(core_id as u8), frame_allocator);
        }
    }
}

#[inline(always)]
pub fn assert_syscall_stack_bounds(core_id: u8) {
    let rsp: u64;
    unsafe { core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack)) };
    let (bottom, top) = syscall_bounds(core_id);
    debug_assert!(
        rsp >= bottom.as_u64() && rsp <= top.as_u64(),
        "syscall RSP {:#x} out of bounds core {} [{:#x}..{:#x}]",
        rsp,
        core_id,
        bottom.as_u64(),
        top.as_u64()
    );
}

const MSR_KERNEL_GS_BASE: u32 = 0xC0000102;

#[repr(C)]
pub struct SyscallFrame {
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,

    pub arg6: u64,        // r9
    pub arg5: u64,        // r8
    pub arg4: u64,        // r10
    pub arg3: u64,        // rdx
    pub arg2: u64,        // rsi
    pub arg1: u64,        // rdi
    pub syscall_num: u64, // rax

    pub rflags: u64,   // r11
    pub user_rip: u64, // rcx
    pub user_rsp: u64, // r15
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_handler() -> ! {
    naked_asm!(
        "swapgs",
        "mov r15, rsp",
        "mov rsp, gs:0",
        "swapgs",

        "push r15",
        "push rcx",
        "push r11",

        "push rax",
        "push rdi",
        "push rsi",
        "push rdx",
        "push r10",
        "push r8",
        "push r9",
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",

        "mov rdi, rsp",

        "call {handle_syscall}",

        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "pop r9",
        "pop r8",
        "pop r10",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "add rsp, 8",

        "pop r11",
        "pop rcx",
        "pop r15",

        "mov rsp, r15",

        "sysretq",

        handle_syscall = sym handle_syscall_inner,
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn handle_syscall_inner(frame: *mut SyscallFrame) -> u64 {
    let frame = unsafe { &mut *frame };
    x86_64::instructions::interrupts::enable();
    {
        let core_id = get_current_core_id();
        assert_syscall_stack_bounds(core_id);
    }

    let syscall = unsafe { core::mem::transmute::<u64, SyscallNumber>(frame.syscall_num) };

    match syscall {
        SyscallNumber::CreateProcess => {
            let elf_path_ptr = frame.arg1 as u64;
            let elf_path_len = frame.arg2 as u64;
            let name_ptr = frame.arg3 as u64;
            let name_len = frame.arg4 as u64;

            let elf_path =
                core::str::from_raw_parts(elf_path_ptr as *const u8, elf_path_len as usize);
            let name = core::str::from_raw_parts(name_ptr as *const u8, name_len as usize);

            serial_println_core!("CreateProcess called");
            serial_println_core!("CreateProcess creating process {} from: {}", name, elf_path);

            match ElfLoadInfo::from_elf_data(&ODYS_ELF) {
                Ok(info) => {
                    serial_println!("  Parsed ELF: entry_point={:#x}", info.entry_point);
                    let mut pm = PROCESS_MANAGER.lock();
                    match pm.create_process_from_elf(0, &info, "proc1", 5) {
                        Ok(init_pid) => {
                            serial_println!(
                                "Init process created (PID {}), added to scheduler",
                                init_pid
                            );
                        }
                        Err(e) => {
                            serial_println!("ERROR: Failed to create init process: {:?}", e);
                            drop(pm);
                            return u64::MAX;
                        }
                    }
                    drop(pm);
                }
                Err(e) => {
                    serial_println!("  ERROR: Failed to parse ELF: {:?}", e);
                    return u64::MAX;
                }
            };

            0u64
        }
        SyscallNumber::TerminateProcess => {
            let target_pid = frame.arg1 as usize;
            let exit_code = frame.arg2 as i32;
            let caller_core = get_current_core_id();
            let caller_pid = scheduler::get_current_process_for_core(caller_core);

            if target_pid == INVALID_PID || target_pid == ARCHE_PID {
                serial_println_core!(
                    "TerminateProcess: invalid target pid {}",
                    target_pid
                );
                return u64::MAX;
            }

            let target_parent = {
                let pm = PROCESS_MANAGER.lock();
                match pm.get_process(target_pid) {
                    Ok(p) => p.parent_pid,
                    Err(_) => {
                        serial_println_core!(
                            "TerminateProcess: target pid {} not found",
                            target_pid
                        );
                        return u64::MAX;
                    }
                }
            };

            let allowed = target_pid == caller_pid
                || target_parent == caller_pid
                || caller_pid == ARCHE_PID;
            if !allowed {
                serial_println_core!(
                    "TerminateProcess: pid {} denied to kill {} (parent {})",
                    caller_pid,
                    target_pid,
                    target_parent
                );
                return u64::MAX;
            }

            for core_id in 0..crate::MAX_CORES {
                let running = scheduler::get_current_process_for_core(core_id);
                if running == target_pid && core_id != caller_core {
                    serial_println_core!(
                        "TerminateProcess: target {} running on core {} busy",
                        target_pid,
                        core_id
                    );
                    return u64::MAX;
                }
            }

            let is_self = target_pid == caller_pid;
            if is_self {
                {
                    let mut pm = PROCESS_MANAGER.lock();
                    if let Ok(proc) = pm.get_process_mut(caller_pid) {
                        proc.execution_context.rip = frame.user_rip;
                        proc.execution_context.rsp = frame.user_rsp;
                        proc.execution_context.rflags = frame.rflags;
                    }
                }
                unsafe {
                    let kernel_phys = crate::memory::get_user_mem_mgr().kernel_page_table_phys;
                    let kf = x86_64::structures::paging::PhysFrame::<x86_64::structures::paging::Size4KiB>::containing_address(
                        kernel_phys,
                    );
                    x86_64::registers::control::Cr3::write(
                        kf,
                        x86_64::registers::control::Cr3Flags::empty(),
                    );
                }
                {
                    let mut pm = PROCESS_MANAGER.lock();
                    if let Err(e) = pm.terminate_process(target_pid, exit_code, false) {
                        serial_println_core!("TerminateProcess self failed: {:?}", e);
                        return u64::MAX;
                    }
                }
                x86_64::instructions::interrupts::disable();
                scheduler::return_to_scheduler();
            } else {
                let res = {
                    let mut pm = PROCESS_MANAGER.lock();
                    pm.terminate_process(target_pid, exit_code, false)
                };
                match res {
                    Ok(()) => {
                        serial_println_core!(
                            "TerminateProcess: pid {} killed {} with code {}",
                            caller_pid,
                            target_pid,
                            exit_code
                        );
                        0
                    }
                    Err(e) => {
                        serial_println_core!("TerminateProcess: kill failed: {:?}", e);
                        u64::MAX
                    }
                }
            }
        }
        SyscallNumber::Write => {
            let fd = frame.arg1;
            let buf = frame.arg2 as *const u8;
            let count = frame.arg3 as usize;
            let slice = unsafe { core::slice::from_raw_parts(buf, count) };
            if let Ok(s) = core::str::from_utf8(slice) {
                if fd == 1 || fd == 2 {
                    let core_id = get_current_core_id();
                    let pid = scheduler::get_current_process_for_core(core_id);
                    crate::serial2_print!("[pid={}] {}", pid, s);
                } else {
                    serial_println_core!("WRITE: fd={}, count={}: {}", fd, count, s);
                }
            }
            count as u64
        }
        SyscallNumber::CreateWindow => {
            let width = frame.arg1 as u32;
            let height = frame.arg2 as u32;
            let x = frame.arg3 as i32;
            let y = frame.arg4 as i32;

            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);

            let mut compositor = crate::graphics::compositor::get_compositor();
            let (window_id, _buffer, _event_buffer) =
                compositor.create_window(width, height, x, y, pid);
            serial_println_core!(
                "sys_create_window: {}x{} at ({},{}) -> id={}",
                width,
                height,
                x,
                y,
                window_id
            );
            compositor.set_z_index(window_id, 7);
            drop(compositor);

            window_id as u64
        }
        SyscallNumber::DestroyWindow => {
            let window_id = frame.arg1 as u32;
            crate::graphics::compositor::get_compositor().destroy_window(window_id);
            serial_println_core!("sys_destroy_window: id={}", window_id);
            0
        }
        SyscallNumber::GetPID => {
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);
            pid as u64
        }
        SyscallNumber::Yield => {
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);
            serial_println_core!("sys_yield: PID {} yielding", pid);

            {
                let mut pm = PROCESS_MANAGER.lock();
                if let Ok(proc) = pm.get_process_mut(pid) {
                    proc.execution_context.rip = frame.user_rip;
                    proc.execution_context.rsp = frame.user_rsp;
                    proc.execution_context.rflags = frame.rflags;
                }
            }

            x86_64::instructions::interrupts::disable();
            scheduler::return_to_scheduler();
        }
        SyscallNumber::Exit => {
            let exit_code = frame.arg1;
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);
            serial_println_core!("sys_exit: PID {} exiting with code {}", pid, exit_code);

            unsafe {
                let kernel_phys = crate::memory::get_user_mem_mgr().kernel_page_table_phys;
                let kf = x86_64::structures::paging::PhysFrame::<x86_64::structures::paging::Size4KiB>::containing_address(
                    kernel_phys,
                );
                x86_64::registers::control::Cr3::write(
                    kf,
                    x86_64::registers::control::Cr3Flags::empty(),
                );
            }
            {
                let mut pm = PROCESS_MANAGER.lock();
                pm.terminate_process(pid, exit_code as i32, false);
            }

            x86_64::instructions::interrupts::disable();
            scheduler::return_to_scheduler();
        }
        SyscallNumber::Allocate => {
            let size = frame.arg1 as usize;
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);

            let old_heap_end = {
                let pm = PROCESS_MANAGER.lock();
                match pm.get_process(pid) {
                    Ok(proc) => proc.memory_layout.heap_end,
                    Err(_) => {
                        serial_println_core!("sys_allocate: pid={} not found", pid);
                        return u64::MAX;
                    }
                }
            };

            let new_heap_end = old_heap_end + size as u64;
            let umm = crate::memory::get_user_mem_mgr();
            let mut fa = crate::memory::get_frame_allocator();

            let result = {
                let mut pm = PROCESS_MANAGER.lock();
                match pm.get_process_mut(pid) {
                    Ok(proc) => match proc.memory_layout.grow_heap(new_heap_end, umm, &mut fa) {
                        Ok(_) => {
                            serial_println_core!(
                                "sys_allocate: pid={} size={} -> ptr={:#x}",
                                pid,
                                size,
                                old_heap_end.as_u64()
                            );
                            old_heap_end.as_u64()
                        }
                        Err(e) => {
                            serial_println_core!(
                                "sys_allocate: pid={} size={} grow_heap failed: {:?}",
                                pid,
                                size,
                                e
                            );
                            u64::MAX
                        }
                    },
                    Err(_) => u64::MAX,
                }
            };
            result
        }
        SyscallNumber::MapWindowBuffer => {
            let window_id = frame.arg1 as u32;
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);

            // User virtual base for window pixel buffers.
            // Chosen to be above the heap and well below the stack.
            const USER_WINDOW_BUFFER_BASE: u64 = 0x0000_0001_0000_0000;
            const MAX_WINDOW_BUFFER_SIZE: u64 = 8 * 1024 * 1024; // 8 MB per window slot

            let user_base = USER_WINDOW_BUFFER_BASE + (window_id as u64) * MAX_WINDOW_BUFFER_SIZE;

            // Retrieve the WindowBuffer Arc from the compositor, then release the compositor lock.
            let buffer_arc = {
                let compositor = crate::graphics::compositor::get_compositor();
                let windows = compositor.windows.read();
                match windows.get(window_id as usize) {
                    Some(w) if w.is_visible => alloc::sync::Arc::clone(&w.buffer),
                    _ => {
                        serial_println_core!(
                            "sys_map_window_buffer: window_id={} not found",
                            window_id
                        );
                        return u64::MAX;
                    }
                }
            };

            let back_vaddr = buffer_arc.back_buffer_virt_addr();
            let front_vaddr = buffer_arc.front_buffer_virt_addr();
            let pixel_count = buffer_arc.pixel_count();
            let byte_count = pixel_count * 4;
            let page_count = (byte_count + 0xFFF) / 0x1000;

            let pml4_phys = {
                let pm = PROCESS_MANAGER.lock();
                match pm.get_process(pid) {
                    Ok(p) => p.memory_layout.top_page_table_phys,
                    Err(_) => {
                        serial_println_core!("sys_map_window_buffer: pid={} not found", pid);
                        return u64::MAX;
                    }
                }
            };

            let umm = crate::memory::get_user_mem_mgr();
            let flags = x86_64::structures::paging::PageTableFlags::PRESENT
                | x86_64::structures::paging::PageTableFlags::WRITABLE
                | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE
                | x86_64::structures::paging::PageTableFlags::NO_EXECUTE;

            {
                let mut frame_allocator = get_frame_allocator();

                for i in 0..page_count {
                    let page_vaddr = back_vaddr + (i * 0x1000) as u64;
                    let front_page_vaddr = front_vaddr + (i * 0x1000) as u64;
                    let phys = umm.translate_kernel_heap_virt_to_phys(page_vaddr);
                    let front_phys = umm.translate_kernel_heap_virt_to_phys(front_page_vaddr);
                    let user_virt = x86_64::VirtAddr::new(user_base + (i * 0x1000) as u64);
                    let front_user_virt = x86_64::VirtAddr::new(
                        user_base + MAX_WINDOW_BUFFER_SIZE + (i * 0x1000) as u64,
                    );

                    if let Err(e) = umm.map_specific_frame(
                        pml4_phys,
                        user_virt,
                        phys,
                        flags,
                        &mut frame_allocator,
                    ) {
                        serial_println_core!(
                            "sys_map_window_buffer: map_specific_frame failed at page {}: {:?}",
                            i,
                            e
                        );
                        return u64::MAX;
                    }
                    if let Err(e) = umm.map_specific_frame(
                        pml4_phys,
                        front_user_virt,
                        front_phys,
                        flags,
                        &mut frame_allocator,
                    ) {
                        serial_println_core!(
                            "sys_map_window_buffer: map_specific_frame failed at page {}: {:?}",
                            i,
                            e
                        );
                        return u64::MAX;
                    }
                }
            }

            serial_println_core!(
                "sys_map_window_buffer: window_id={} mapped {} pages at user {:#x}",
                window_id,
                page_count,
                user_base
            );
            user_base
        }
        SyscallNumber::PresentWindow => {
            let window_id = frame.arg1 as u32;
            let compositor = crate::graphics::compositor::get_compositor();
            let windows = compositor.windows.read();
            if let Some(w) = windows.get(window_id as usize) {
                if w.is_visible {
                    w.buffer.present();
                }
            }
            //serial_println_core!("sys_present_window: window_id={} presented", window_id);
            drop(windows);
            drop(compositor);
            0
        }
        SyscallNumber::GetWindowSize => {
            let window_id = frame.arg1 as u32;
            let compositor = crate::graphics::compositor::get_compositor();
            let windows = compositor.windows.read();
            match windows.get(window_id as usize) {
                Some(w) if w.is_visible => {
                    let width = w.buffer.width as u64;
                    let height = w.buffer.height as u64;
                    (width << 32) | height
                }
                _ => u64::MAX,
            }
        }
        SyscallNumber::GetWindowInfo => {
            let window_id = frame.arg1 as u32;
            let info_ptr = frame.arg2 as usize;

            serial_println_core!("syscall: GetWindowInfo: window_id={}", window_id);

            if !validate_user_ptr(info_ptr, core::mem::size_of::<WindowInfo>()) {
                return FileDescriptor::INVALID_FD;
            }

            let compositor = crate::graphics::compositor::get_compositor();
            let windows = compositor.windows.read();
            match windows.get(window_id as usize) {
                Some(w) => {
                    let out = unsafe { &mut *(info_ptr as *mut WindowInfo) };
                    out.width = w.buffer.width;
                    out.height = w.buffer.height;
                    out.event_buffer_vaddr = match w.event_buffer {
                        Some(buffer) => buffer as *const EventBuffer as u64,
                        None => 0,
                    }
                }
                _ => {
                    return u64::MAX;
                }
            }

            0
        }
        SyscallNumber::FocusWindow => {
            let window_id = frame.arg1 as u32;
            crate::graphics::compositor::get_compositor().focus_window(window_id);
            serial_println_core!("sys_focus_window: id={}", window_id);
            0
        }

        SyscallNumber::OpenFile => {
            let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                Some(s) => s,
                None => return FileDescriptor::INVALID_FD,
            };
            let flags = frame.arg3 as u8;

            let node_id: usize = {
                let mut sirius_guard = crate::filesystem::sirius::get_sirius();
                match sirius_guard.resolve_path(&path) {
                    Ok(node) => node.node_id,
                    Err(e) => {
                        serial_println_core!("sys_open_file: '{}' not found: {:?}", path, e);
                        return FileDescriptor::INVALID_FD;
                    }
                }
            };

            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);

            let fd_index = {
                let mut pm = PROCESS_MANAGER.lock();
                match pm.get_process_mut(pid) {
                    Ok(proc) => {
                        let idx = proc.file_descriptors.size;
                        proc.file_descriptors
                            .push(FileDescriptor { node_id, flags });
                        idx
                    }
                    Err(_) => return FileDescriptor::INVALID_FD,
                }
            };

            serial_println_core!("sys_open_file: '{}' -> fd={}", path, fd_index);
            fd_index as u64
        }

        SyscallNumber::CloseFile => {
            let fd = frame.arg1 as usize;
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);

            let mut pm = PROCESS_MANAGER.lock();
            match pm.get_process_mut(pid) {
                Ok(proc) => {
                    if fd >= proc.file_descriptors.size {
                        return FileDescriptor::INVALID_FD;
                    }
                    let last = proc.file_descriptors.size - 1;
                    if fd != last {
                        let last_fd = *proc.file_descriptors.get(last);
                        *proc.file_descriptors.get_mut(fd) = last_fd;
                    }
                    proc.file_descriptors.size -= 1;
                    serial_println_core!("sys_closeFile: fd={} closed", fd);
                    0
                }
                Err(_) => FileDescriptor::INVALID_FD,
            }
        }

        SyscallNumber::ReadFile => {
            let fd = frame.arg1 as usize;
            let offset = frame.arg2 as usize;
            let buffer_ptr = frame.arg3 as usize;
            let count = frame.arg4 as usize;
            serial_println_core!(
                "sys_read_file: fd={} offset={} buffer_ptr={:p} count={}",
                fd,
                offset,
                buffer_ptr as *const u8,
                count
            );

            if !validate_user_ptr(buffer_ptr, count) {
                return FileDescriptor::INVALID_FD;
            }

            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);

            let (node_id, flags) = {
                let pm = PROCESS_MANAGER.lock();
                match pm.get_process(pid) {
                    Ok(proc) => {
                        if fd >= proc.file_descriptors.size {
                            return FileDescriptor::INVALID_FD;
                        }
                        let d = proc.file_descriptors.get(fd);
                        (d.node_id, d.flags)
                    }
                    Err(_) => return FileDescriptor::INVALID_FD,
                }
            };

            if flags & crate::process::process::FD_FLAG_READ == 0 {
                return FileDescriptor::INVALID_FD;
            }

            let buffer = unsafe { core::slice::from_raw_parts_mut(buffer_ptr as *mut u8, count) };

            let bytes_read = {
                let mut sirius = crate::filesystem::sirius::get_sirius();

                #[cfg(feature = "use_cached_fs")]
                {
                    match sirius.driver.read_file(node_id, offset, buffer) {
                        Ok(n) => n,
                        Err(e) => {
                            serial_println_core!("sys_read_file: fd={} error: {:?}", fd, e);
                            return FileDescriptor::INVALID_FD;
                        }
                    }
                }

                #[cfg(not(feature = "use_cached_fs"))]
                {
                    match sirius.driver.read_file(node_id, offset, buffer) {
                        Ok(n) => n,
                        Err(e) => {
                            serial_println_core!("sys_read_file: fd={} error: {:?}", fd, e);
                            return FileDescriptor::INVALID_FD;
                        }
                    }
                }
            };

            bytes_read as u64
        }

        SyscallNumber::WriteFile => {
            let fd = frame.arg1 as usize;
            let offset = frame.arg2 as usize;
            let buffer_ptr = frame.arg3 as usize;
            let count = frame.arg4 as usize;

            if !validate_user_ptr(buffer_ptr, count) {
                return FileDescriptor::INVALID_FD;
            }

            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);

            let (node_id, flags) = {
                let pm = PROCESS_MANAGER.lock();
                match pm.get_process(pid) {
                    Ok(proc) => {
                        if fd >= proc.file_descriptors.size {
                            return FileDescriptor::INVALID_FD;
                        }
                        let descriptor = proc.file_descriptors.get(fd);
                        (descriptor.node_id, descriptor.flags)
                    }
                    Err(_) => return FileDescriptor::INVALID_FD,
                }
            };

            if flags & crate::process::process::FD_FLAG_WRITE == 0 {
                return FileDescriptor::INVALID_FD;
            }

            let buffer = unsafe { core::slice::from_raw_parts(buffer_ptr as *const u8, count) };
            let bytes_written = {
                let mut sirius = crate::filesystem::sirius::get_sirius();
                match sirius.driver.write_file(node_id, offset, buffer) {
                    Ok(n) => n,
                    Err(e) => {
                        serial_println_core!("sys_write_file: fd={} error: {:?}", fd, e);
                        return FileDescriptor::INVALID_FD;
                    }
                }
            };

            // {
            //     let mut pm = PROCESS_MANAGER.lock();
            //     if let Ok(proc) = pm.get_process_mut(pid) {
            //         if fd < proc.file_descriptors.size {
            //             proc.file_descriptors.get_mut(fd).offset += bytes_written;
            //         }
            //     }
            // }

            bytes_written as u64
        }

        SyscallNumber::StatFile => {
            let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                Some(s) => s,
                None => return FileDescriptor::INVALID_FD,
            };
            let stat_ptr = frame.arg3 as usize;

            if !validate_user_ptr(stat_ptr, core::mem::size_of::<StatFlat>()) {
                return FileDescriptor::INVALID_FD;
            }

            let node = {
                let mut sirius = crate::filesystem::sirius::get_sirius();
                match sirius.resolve_path(&path) {
                    Ok(file_node) => file_node,
                    Err(e) => {
                        serial_println_core!("sys_stat: '{}' error: {:?}", path, e);
                        return FileDescriptor::INVALID_FD;
                    }
                }
            };

            let out = unsafe { &mut *(stat_ptr as *mut StatFlat) };
            out.name = [0u8; FS_NAME_LEN];
            let name_bytes = node.name.as_bytes();
            let copy_len = name_bytes.len().min(FS_NAME_LEN);
            out.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
            out.name_len = copy_len as u8;
            out.is_dir = (node.file_type == crate::filesystem::sirius::FileType::Directory) as u8;
            out.size = node.size as u64;
            out.created_time = node.created_time;
            out.modified_time = node.modified_time;

            serial_println_core!("sys_stat: {}, size: {}", path, out.size);

            0
        }

        SyscallNumber::ListDir => {
            let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                Some(s) => s,
                None => return FileDescriptor::INVALID_FD,
            };
            let out_ptr = frame.arg3 as usize;
            let out_buffer_len = frame.arg4 as usize;

            if !validate_user_ptr(out_ptr, out_buffer_len) {
                return FileDescriptor::INVALID_FD;
            }

            let entries = {
                let mut sirius = crate::filesystem::sirius::get_sirius();
                match sirius.list_directory(&path) {
                    Ok(v) => v,
                    Err(e) => {
                        serial_println_core!("sys_list_dir: '{}' error: {:?}", path, e);
                        return FileDescriptor::INVALID_FD;
                    }
                }
            };

            let entry_size = core::mem::size_of::<DirEntryFlat>();
            let max_entries = out_buffer_len / entry_size;
            let write_count = entries.len().min(max_entries);

            for (i, node) in entries.iter().take(write_count).enumerate() {
                let slot_ptr = (out_ptr + i * entry_size) as *mut DirEntryFlat;
                let slot = unsafe { &mut *slot_ptr };
                slot.name = [0u8; FS_NAME_LEN];
                let name_bytes = node.name.as_bytes();
                let copy_len = name_bytes.len().min(FS_NAME_LEN);
                slot.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
                slot.name_len = copy_len as u8;
                slot.is_dir =
                    (node.file_type == crate::filesystem::sirius::FileType::Directory) as u8;
                slot.size = node.size as u64;
                slot.created_time = node.created_time;
                slot.modified_time = node.modified_time;
            }

            write_count as u64
        }

        SyscallNumber::CreateFile => {
            let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                Some(s) => s,
                None => return FileDescriptor::INVALID_FD,
            };

            let mut sirius = crate::filesystem::sirius::get_sirius();
            match sirius.create_file(&path) {
                Ok(_) => {
                    serial_println_core!("sys_create_file: '{}' created", path);
                    0
                }
                Err(e) => {
                    serial_println_core!("sys_create_file: '{}' error: {:?}", path, e);
                    FileDescriptor::INVALID_FD
                }
            }
        }

        SyscallNumber::CreateDir => {
            let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                Some(s) => s,
                None => return FileDescriptor::INVALID_FD,
            };

            let mut sirius = crate::filesystem::sirius::get_sirius();
            match sirius.create_directory(&path) {
                Ok(_) => {
                    serial_println_core!("sys_create_dir: '{}' created", path);
                    0
                }
                Err(e) => {
                    serial_println_core!("sys_create_dir: '{}' error: {:?}", path, e);
                    FileDescriptor::INVALID_FD
                }
            }
        }

        SyscallNumber::Delete => {
            let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                Some(s) => s,
                None => return FileDescriptor::INVALID_FD,
            };

            let mut sirius = crate::filesystem::sirius::get_sirius();
            match sirius.delete(&path) {
                Ok(_) => {
                    serial_println_core!("sys_delete: '{}' deleted", path);
                    0
                }
                Err(e) => {
                    serial_println_core!("sys_delete: '{}' error: {:?}", path, e);
                    FileDescriptor::INVALID_FD
                }
            }
        }

        SyscallNumber::PinFile => {
            #[cfg(not(feature = "use_cached_fs"))]
            {
                serial_println_core!("sys_pin_file: cache not enabled");
                return u64::MAX;
            }

            #[cfg(feature = "use_cached_fs")]
            {
                let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                    Some(s) => s,
                    None => return u64::MAX,
                };

                let mut sirius = crate::filesystem::sirius::get_sirius();
                match sirius.pin_file(&path) {
                    Ok(()) => {
                        serial_println_core!("sys_pin_file: '{}' pinned", path);
                        0
                    }
                    Err(e) => {
                        serial_println_core!("sys_pin_file: '{}' error: {:?}", path, e);
                        u64::MAX
                    }
                }
            }
        }

        SyscallNumber::UnpinFile => {
            #[cfg(not(feature = "use_cached_fs"))]
            {
                serial_println_core!("sys_unpin_file: cache not enabled");
                return u64::MAX;
            }

            #[cfg(feature = "use_cached_fs")]
            {
                let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                    Some(s) => s,
                    None => return u64::MAX,
                };

                let mut sirius = crate::filesystem::sirius::get_sirius();
                match sirius.unpin_file(&path) {
                    Ok(()) => {
                        serial_println_core!("sys_unpin_file: '{}' unpinned", path);
                        0
                    }
                    Err(e) => {
                        serial_println_core!("sys_unpin_file: '{}' error: {:?}", path, e);
                        u64::MAX
                    }
                }
            }
        }

        SyscallNumber::ReserveCache => {
            #[cfg(not(feature = "use_cached_fs"))]
            {
                serial_println_core!("sys_reserve_cache: cache not enabled");
                return u64::MAX;
            }

            #[cfg(feature = "use_cached_fs")]
            {
                let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                    Some(s) => s,
                    None => return u64::MAX,
                };

                let importance = match frame.arg3 {
                    0 => CacheImportance::Minimal,
                    1 => CacheImportance::Low,
                    2 => CacheImportance::Normal,
                    3 => CacheImportance::High,
                    4 => CacheImportance::VeryHigh,
                    5 => CacheImportance::Critical,
                    6 => CacheImportance::Resident,
                    _ => {
                        serial_println_core!(
                            "sys_reserve_cache: invalid importance {}",
                            frame.arg3
                        );
                        return u64::MAX;
                    }
                };

                let mut sirius = crate::filesystem::sirius::get_sirius();
                match sirius.reserve_cache(&path, importance) {
                    Ok(()) => {
                        serial_println_core!(
                            "sys_reserve_cache: '{}' reserved with importance {}",
                            path,
                            importance
                        );
                        0
                    }
                    Err(e) => {
                        serial_println_core!("sys_reserve_cache: '{}' error: {:?}", path, e);
                        u64::MAX
                    }
                }
            }
        }

        SyscallNumber::EvictDirectory => {
            #[cfg(not(feature = "use_cached_fs"))]
            {
                serial_println_core!("sys_evict_directory: cache not enabled");
                return u64::MAX;
            }

            #[cfg(feature = "use_cached_fs")]
            {
                let path = match read_user_string(frame.arg1 as usize, frame.arg2 as usize) {
                    Some(s) => s,
                    None => return u64::MAX,
                };

                let mut sirius = crate::filesystem::sirius::get_sirius();
                match sirius.evict_directory(&path) {
                    Ok(freed) => {
                        serial_println_core!(
                            "sys_evict_directory: '{}' evicted, freed {} bytes",
                            path,
                            freed
                        );
                        freed as u64
                    }
                    Err(e) => {
                        serial_println_core!("sys_evict_directory: '{}' error: {:?}", path, e);
                        u64::MAX
                    }
                }
            }
        }

        SyscallNumber::GetCacheStats => {
            #[cfg(not(feature = "use_cached_fs"))]
            {
                0
            }

            #[cfg(feature = "use_cached_fs")]
            {
                let stats_ptr = frame.arg1 as usize;

                if !validate_user_ptr(stats_ptr, core::mem::size_of::<CacheStatsFlat>()) {
                    return u64::MAX;
                }

                let sirius = crate::filesystem::sirius::get_sirius();
                let stats = sirius.cache_stats();
                let flat = CacheStatsFlat {
                    total_files: stats.total_files as u64,
                    total_bytes: stats.total_bytes as u64,
                    max_bytes: stats.max_bytes as u64,
                    dirty_files: stats.dirty_files as u64,
                };

                unsafe {
                    core::ptr::write(stats_ptr as *mut CacheStatsFlat, flat);
                }

                serial_println_core!(
                    "sys_get_cache_stats: {} files, {} / {} bytes",
                    stats.total_files,
                    stats.total_bytes,
                    stats.max_bytes
                );
                0
            }
        }

        SyscallNumber::FlushFileCache => {
            #[cfg(not(feature = "use_cached_fs"))]
            {
                0
            }

            #[cfg(feature = "use_cached_fs")]
            {
                let core_id = get_current_core_id();
                let pid = scheduler::get_current_process_for_core(core_id);

                let node_ids: Vec<crate::filesystem::fat32::FileNodeHandle> = {
                    let pm = PROCESS_MANAGER.lock();
                    match pm.get_process(pid) {
                        Ok(proc) => proc
                            .file_descriptors
                            .iter()
                            .map(|fd| fd.node_id as crate::filesystem::fat32::FileNodeHandle)
                            .collect(),
                        Err(_) => return u64::MAX,
                    }
                };

                let mut sirius = crate::filesystem::sirius::get_sirius();
                match sirius.flush_nodes(&node_ids) {
                    Ok(()) => {
                        serial_println_core!(
                            "sys_flush_file_cache: pid={} flushed {} nodes",
                            pid,
                            node_ids.len()
                        );
                        0
                    }
                    Err(e) => {
                        serial_println_core!("sys_flush_file_cache: pid={} error: {:?}", pid, e);
                        u64::MAX
                    }
                }
            }
        }

        SyscallNumber::CreateCircularBuffer => {
            let size_bytes = frame.arg1 as usize;
            let page_flags = PageTableFlags::from_bits_truncate(frame.arg2);
            let buffer_info_out = frame.arg3 as *mut CircularBufferInfo;
            
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);
            
            let mut frame_allocator = get_frame_allocator();
            let user_memory_manager = &crate::memory::get_user_mem_mgr();
            let mut pm = crate::process::process_manager::PROCESS_MANAGER.lock();
            
            if let Ok(proc) = pm.get_process_mut(pid) {
                serial_println_core!(
                    "CreateCircularBuffer: found process for pid: {}",
                    pid
                );

                match CircularBuffer::map_for_user(size_bytes, page_flags as PageTableFlags, user_memory_manager, &mut proc.memory_layout, &mut frame_allocator)
                {
                    Some(buffer_info) => {
                        serial_println_core!("CreateCircularBuffer: buffer created, virt_base: {}, view_size: {}", buffer_info.virtual_base, buffer_info.view_size);
                        *buffer_info_out = buffer_info;
                        return 0;
                    }
                    None => {
                        serial_println_core!("CreateCircularBuffer: cant create the buffer");
                        return u64::MAX;
                    }
                }
            }

            u64::MAX
        },

        SyscallNumber::GetCpuInfo => {
            let buffer_ptr = frame.arg1 as usize;
            let buffer_len = frame.arg2 as usize;

            if !validate_user_ptr(buffer_ptr, buffer_len) {
                serial_println_core!("sys_get_cpu_info: invalid pointer");
                return 2;
            }

            let core_id = get_current_core_id();
            let cpu_info = get_cpu_info_for_core(core_id);

            let tsc_freq = TSC_FREQUENCY_HZ.load(Ordering::Relaxed);
            let boot_tsc = BOOT_TSC.load(Ordering::Relaxed);
            let core_count = unsafe { TOTAL_CORE_COUNT };

            let flat = CpuInfoFlat::from_kernel_info(cpu_info, tsc_freq, boot_tsc, core_count);

            unsafe {
                core::ptr::write(buffer_ptr as *mut CpuInfoFlat, flat);
            }

            0
        }

        _ => u64::MAX,
    }
}

#[cfg(feature = "use_cached_fs")]
#[repr(C)]
pub struct CacheStatsFlat {
    pub total_files: u64,
    pub total_bytes: u64,
    pub max_bytes: u64,
    pub dirty_files: u64,
}

pub fn init_syscall() {
    let core_id = get_current_core_id() as usize;
    debug_assert!(core_id < crate::MAX_CORES as usize);

    let stack_top = unsafe {
        let slot = &mut SYSCALL_SLOTS[core_id].data;
        let stack_end_ptr = slot._stack.as_ptr().add(SYSCALL_STACK_SIZE);
        let top = stack_end_ptr as u64;
        slot.stack_top = top;
        top
    };

    let slot_addr = unsafe { &SYSCALL_SLOTS[core_id].data as *const _ as u64 };
    unsafe {
        msr_write(MSR_KERNEL_GS_BASE, slot_addr);
    }

    serial_println_core!("Syscall stack top (core {}): {:#x}", core_id, stack_top);

    unsafe {
        Efer::update(|flags| {
            flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
        });
    }

    // STAR MSR layout:
    // https://www.felixcloutier.com/x86/sysret
    // Bits 63:48 = User CS base for sysretq
    // Bits 47:32 = Kernel CS base for syscall
    //
    // GDT layout:
    //   0x08 = kernel code, 0x10 = kernel data
    //   0x18 = user code, 0x20 = user data
    //
    // sysretq sets CS = (STAR[63:48] + 16) | 3 = 0x23, SS = (STAR[63:48] + 8) | 3 = 0x1B
    let star_value = (0x10u64 << 48) | (0x08u64 << 32);
    let sfmask: u64 = 1 << 9;
    unsafe {
        msr_write(0xC0000081, star_value);
        msr_write(0xC0000082, syscall_handler as *const () as u64);
        msr_write(0xC0000084, sfmask);
    }

    serial_println_core!("Syscall MSRs initialized");
}

/// Flush all dirty cached files to disk. Call this from the kernel whenever a
/// global writeback is needed (e.g., before shutdown or unmount).
#[cfg(feature = "use_cached_fs")]
pub fn flush_all_file_caches() {
    let mut sirius = crate::filesystem::sirius::get_sirius();
    if let Err(e) = sirius.flush_all() {
        serial_println_core!("flush_all_file_caches: error: {:?}", e);
    } else {
        serial_println_core!("flush_all_file_caches: all dirty files flushed");
    }
}
