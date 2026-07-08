use crate::serial_println;
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
use core::arch::naked_asm;
use x86_64::registers::model_specific::{Efer, EferFlags};

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

pub enum SystemCall {
    CreateProcess {
        parent_pid: usize,
        name_ptr: *const u8,
        name_len: u8,
        is_out: bool,
    },
    TerminateProcess {
        pid_to_kill: usize,
        exit_code: i32,
        kill_children: bool,
    },
    Write {
        fd: usize,
        buffer_ptr: usize,
        n_bytes: usize,
    },
    Read {
        fd: usize,
        buffer_ptr: usize,
        n_bytes: usize,
    },
    GetLine {
        fd: usize,
        buffer_ptr: usize,
        n_bytes: usize,
    },
    Allocate {
        size: usize,
    },
    CreateFile {
        path_ptr: usize,
        path_len: usize,
    },
    RemoveFile {
        path_ptr: usize,
        path_len: usize,
    },
    LoadFile {
        path_ptr: usize,
        path_len: usize,
    },
    UnloadFile {
        fd: usize,
    },
    CreateWindow {
        process_id: usize,
    },
    GetProcessInfo {
        pid: usize,
    },
    Exit {
        return_code: u32,
    },
}

#[repr(u64)]
pub enum SyscallNumber {
    CreateProcess = 0,
    TerminateProcess = 1,
    Write = 2,
    Read = 3,
    GetLine = 4,
    Allocate = 5,
    CreateFile = 6,
    RemoveFile = 7,
    LoadFile = 8,
    UnloadFile = 9,
    CreateWindow = 10,
    DestroyWindow = 11,
    MapWindowBuffer = 12,
    PresentWindow = 13,
    GetWindowSize = 14,
    FocusWindow = 15,
    GetProcessInfo = 996,
    GetPID = 997,
    Yield = 998,
    Exit = 999,
}

impl SystemCall {
    pub fn from_number_and_args(
        num: usize,
        arg1: usize,
        arg2: usize,
        arg3: usize,
        arg4: usize,
        _arg5: usize,
        _arg6: usize,
    ) -> Option<Self> {
        match num {
            0 => Some(SystemCall::CreateProcess {
                parent_pid: arg1,
                name_ptr: arg2 as *const u8,
                name_len: arg3 as u8,
                is_out: arg4 != 0,
            }),
            1 => Some(SystemCall::TerminateProcess {
                pid_to_kill: arg1,
                exit_code: arg2 as i32,
                kill_children: arg3 != 0,
            }),
            2 => Some(SystemCall::Write {
                fd: arg1,
                buffer_ptr: arg2,
                n_bytes: arg3,
            }),
            3 => Some(SystemCall::Read {
                fd: arg1,
                buffer_ptr: arg2,
                n_bytes: arg3,
            }),
            4 => Some(SystemCall::GetLine {
                fd: arg1,
                buffer_ptr: arg2,
                n_bytes: arg3,
            }),
            5 => Some(SystemCall::Allocate { size: arg1 }),
            999 => Some(SystemCall::Exit {
                return_code: arg1 as u32,
            }),
            _ => None,
        }
    }
}

pub fn handle_syscall(pid: usize, call: SystemCall) -> Result<(), SyscallError> {
    assert!(pid != INVALID_PID);

    let mut pm = PROCESS_MANAGER.lock();

    match call {
        SystemCall::CreateProcess {
            parent_pid,
            name_ptr,
            name_len,
            is_out,
        } => {
            let priority = 0;
            if pid != parent_pid && pid != ARCHE_PID {
                return Err(SyscallError::PermissionDenied);
            }
            // TODO: entry_point, stack_top, and page_table_base should come from ELF loader
            match pm.create_process(
                parent_pid, priority, name_ptr, name_len, is_out,
                0, // entry_point (placeholder - will be set by ELF loader)
                0, // stack_top (placeholder - will be set by ELF loader)
                0, // page_table_base (placeholder - will be set by ELF loader)
            ) {
                Ok(new_pid) => {
                    serial_println!("Created process with PID: {}", new_pid);
                    Ok(())
                }
                Err(e) => {
                    serial_println!("Failed to create process: {:?}", e);
                    Err(SyscallError::ProcessNotFound)
                }
            }
        }
        SystemCall::TerminateProcess {
            pid_to_kill,
            exit_code,
            kill_children,
        } => {
            if pid != pid_to_kill {
                return Err(SyscallError::PermissionDenied);
            }
            match pm.terminate_process(pid_to_kill, exit_code, kill_children) {
                Ok(_) => {
                    serial_println!("Terminated process, PID: {}", pid_to_kill);
                    Ok(())
                }
                Err(e) => {
                    serial_println!("Failed to terminate process: {:?}", e);
                    Err(SyscallError::ProcessNotFound)
                }
            }
        }

        _ => Err(SyscallError::SyscallNotFound),
    }
}

const SYSCALL_STACK_SIZE: usize = 4096 * 16; // 64 KiB per core

// First field must be the stack top pointer — the naked asm reads gs:0.
// Repr(C) guarantees field order. Align to cache line to avoid false sharing.
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

static mut PER_CORE_SYSCALL: [PerCoreSyscallData; crate::MAX_CORES as usize] = {
    const EMPTY: PerCoreSyscallData = PerCoreSyscallData::zeroed();
    [EMPTY; crate::MAX_CORES as usize]
};

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
        // on syscall entry: CS/SS switched, interrupts off, user RIP in RCX, RFLAGS in R11
        // swap to kernel GS
        "swapgs",

        // save user RSP; load per-core kernel stack top from gs:0 (stack_top field)
        "mov r15, rsp",
        "mov rsp, gs:0",

        // restore user GS now that we are on the kernel stack and no longer need gs:0
        "swapgs",

        // save state on kernel stack
        "push r15", // user RSP
        "push rcx", // user RIP
        "push r11", // user RFLAGS

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
        "add rsp, 8", // pop rax
        //"add rsp, 7*8",

        "pop r11",
        "pop rcx",
        "pop r15",

        // back to user stack
        "mov rsp, r15",

        "sysretq",

        handle_syscall = sym handle_syscall_inner,
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn handle_syscall_inner(frame: *mut SyscallFrame) -> u64 {
    let frame = unsafe { &mut *frame };

    // SFMASK masked IF on syscall entry to prevent the timer firing while SS
    // still holds the kernel selector. Re-enable interrupts now that we are on
    // the per-core kernel stack so the timer can preempt long-running syscalls.
    // sysretq will restore RFLAGS from R11 (user RFLAGS, IF=1), so interrupts
    // stay enabled on return to userspace without any extra work here.
    x86_64::instructions::interrupts::enable();

    // serial_println_core!(
    //     "Syscall: num={}, arg1={:#x}, arg2={:#x}, arg3={:#x}, arg4={:#x}, arg5={:#x}, arg6={:#x}",
    //     frame.syscall_num,
    //     frame.arg1,
    //     frame.arg2,
    //     frame.arg3,
    //     frame.arg4,
    //     frame.arg5,
    //     frame.arg6,
    // );

    let syscall = unsafe { core::mem::transmute::<u64, SyscallNumber>(frame.syscall_num) };

    match syscall {
        SyscallNumber::Write => {
            let fd = frame.arg1;
            let buf = frame.arg2 as *const u8;
            let count = frame.arg3 as usize;
            // serial_println_core!("WRITE: fd={}, count={}", fd, count);
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

            let mut compositor = crate::graphics::compositor::get_compositor();
            let (window_id, _buffer) = compositor.create_window(width, height, x, y);
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
        // Voluntarily yield the CPU back to the scheduler without terminating
        // Save the userspace return address and stack into the process's execution_context
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

            // Disable interrupts before unwinding to the scheduler stack.
            // The timer must not fire between here and return_to_scheduler's ret.
            x86_64::instructions::interrupts::disable();
            scheduler::return_to_scheduler();
        }
        SyscallNumber::Exit => {
            let exit_code = frame.arg1;
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);
            serial_println_core!("sys_exit: PID {} exiting with code {}", pid, exit_code);

            {
                let mut pm = PROCESS_MANAGER.lock();
                pm.terminate_process(pid, exit_code as i32, false);
            }

            // Disable interrupts before unwinding to the scheduler stack.
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

            for i in 0..page_count {
                let page_vaddr = back_vaddr + (i * 0x1000) as u64;
                let phys = umm.translate_kernel_heap_virt_to_phys(page_vaddr);
                let user_virt = x86_64::VirtAddr::new(user_base + (i * 0x1000) as u64);

                if let Err(e) = umm.map_specific_frame(pml4_phys, user_virt, phys, flags) {
                    serial_println_core!(
                        "sys_map_window_buffer: map_specific_frame failed at page {}: {:?}",
                        i,
                        e
                    );
                    return u64::MAX;
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
            serial_println_core!(
                "sys_present_window: window_id={} presented",
                window_id
            );
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
        SyscallNumber::FocusWindow => {
            let window_id = frame.arg1 as u32;
            crate::graphics::compositor::get_compositor().focus_window(window_id);
            serial_println_core!("sys_focus_window: id={}", window_id);
            0
        }
        _ => u64::MAX,
    }
}

pub fn init_syscall() {
    let core_id = get_current_core_id() as usize;
    debug_assert!(core_id < crate::MAX_CORES as usize);

    // Compute the stack top for this core and store it in the per-core slot.
    // Stack grows downward: top = address of byte just past the _stack array.
    let stack_top = unsafe {
        let slot = &mut PER_CORE_SYSCALL[core_id];
        let stack_end_ptr = slot._stack.as_ptr().add(SYSCALL_STACK_SIZE);
        let top = stack_end_ptr as u64;
        slot.stack_top = top;
        top
    };

    // Write the address of this core's PerCoreSyscallData into KERNEL_GS_BASE.
    // On syscall entry, swapgs makes GS point here, so gs:0 == stack_top.
    let slot_addr = unsafe { &PER_CORE_SYSCALL[core_id] as *const _ as u64 };
    unsafe {
        msr_write(MSR_KERNEL_GS_BASE, slot_addr);
    }

    serial_println_core!("Syscall stack top (core {}): {:#x}", core_id, stack_top);

    // Enable SYSCALL/SYSRET via EFER.SCE
    unsafe {
        Efer::update(|flags| {
            flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
        });
    }

    // STAR MSR layout:
    // https://www.felixcloutier.com/x86/sysret
    // Bits 63:48 = User CS base for sysretq (CS = this + 16, SS = this + 8)
    // Bits 47:32 = Kernel CS base for syscall (CS = this, SS = this + 8)
    //
    // GDT layout:
    //   0x08 = kernel code, 0x10 = kernel data
    //   0x18 = user data, 0x20 = user code
    //
    // sysretq sets CS = (STAR[63:48] + 16) | 3 = 0x23, SS = (STAR[63:48] + 8) | 3 = 0x1B
    let star_value = (0x10u64 << 48) | (0x08u64 << 32);
    // SFMASK: mask these RFLAGS bits on syscall entry.
    // Bit 9 (IF) must be masked so the timer cannot fire mid-syscall while SS
    // is still the kernel selector. sysretq restores RFLAGS from R11 (saved
    // user RFLAGS with IF=1), so interrupts re-enable automatically on return
    // to userspace. We re-enable interrupts manually inside handle_syscall_inner
    // for preemption of long-running syscalls.
    let sfmask: u64 = 1 << 9; // mask IF
    unsafe {
        msr_write(0xC0000081, star_value);
        msr_write(0xC0000082, syscall_handler as *const () as u64);
        msr_write(0xC0000084, sfmask);
    }

    serial_println_core!("Syscall MSRs initialized");
}
