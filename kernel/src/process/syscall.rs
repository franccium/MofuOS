use crate::{process::{
    SCHEDULER, process::INVALID_PID, process_manager::{ARCHE_PID, PROCESS_MANAGER}, scheduler
}, serial_println_core, util::cpuinfo::get_current_core_id};
use crate::serial_println;
use crate::util::msr::{msr_write};
use core::arch::naked_asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::{
    VirtAddr,
    registers::model_specific::{Efer, EferFlags},
};

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

#[repr(usize)]
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
    GetProcessInfo = 11,
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
                parent_pid,
                priority,
                name_ptr,
                name_len,
                is_out,
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

//TODO: fix stacks
//TODO: make per-core
const SYSCALL_STACK_SIZE: usize = 4096 * 16;
#[repr(align(4096))]
struct SyscallStack([u8; SYSCALL_STACK_SIZE]);
static mut SYSCALL_STACK: SyscallStack = SyscallStack([0; SYSCALL_STACK_SIZE]);
static STACK_TOP: AtomicU64 = AtomicU64::new(0);


pub fn init_syscall_stack() {
    let top = unsafe {
        let stack_bottom = core::ptr::addr_of!(SYSCALL_STACK).cast::<u8>();
        let stack_top_ptr = stack_bottom.add(SYSCALL_STACK_SIZE);
        VirtAddr::from_ptr(stack_top_ptr).as_u64()
    };
    STACK_TOP.store(top, Ordering::SeqCst);
    serial_println!("Syscall stack top: {:#x}", top);
}

#[repr(C)]
pub struct SyscallFrame {
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbx: u64,
    pub rbp: u64,

    pub arg6: u64, // r9
    pub arg5: u64, // r8
    pub arg4: u64, // r10
    pub arg3: u64, // rdx
    pub arg2: u64, // rsi
    pub arg1: u64, // rdi
    pub syscall_num: u64, // rax

    pub rflags: u64, // r11
    pub user_rip: u64, // rcx
    pub user_rsp: u64, // r15
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_handler() -> ! {
    naked_asm!(
        // save user stack
        "mov r15, rsp",

        // switch to kernel stack
        "mov rsp, qword ptr [rip + {stack_top}]",

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

        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",

        "mov rdi, rsp",

        "call {handle_syscall}",

        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",

        "add rsp, 7*8",

        "pop r11",
        "pop rcx",
        "pop r15",

        // back to user stack
        "mov rsp, r15",

        "sysretq",

        stack_top = sym STACK_TOP,
        handle_syscall = sym handle_syscall_inner,
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn handle_syscall_inner(frame: *mut SyscallFrame) -> u64 {
    let frame = unsafe { &mut *frame };

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

    match frame.syscall_num {
        2 => {
            let fd = frame.arg1;
            let buf = frame.arg2 as *const u8;
            let count = frame.arg3 as usize;
            serial_println_core!("WRITE: fd={}, buf={:p}, count={}", fd, buf, count);

            let slice = unsafe { core::slice::from_raw_parts(buf, count) };
            if let Ok(s) = core::str::from_utf8(slice) {
                crate::serial_print!("{}", s);
            }
            count as u64
        }
        997 => {
            // serial_println_core!("997 returning: {}", frame.arg1);
            frame.arg1
        }
        999 => {
            let exit_code = frame.arg1;
            serial_println_core!("Process exited with code: {}", exit_code);
            let core_id = get_current_core_id();
            let pid = scheduler::get_current_process_for_core(core_id);
            serial_println_core!("Exiting process' PID: {}", pid);

            {
                let mut pm = PROCESS_MANAGER.lock();
                pm.terminate_process(pid, exit_code as i32, false);
            }

            {
                //let mut scheduler = SCHEDULER.lock();
                //TODO: remove from here as well?
            }

            //TODO: return to scheduler

            loop {
                x86_64::instructions::hlt();
            }
            exit_code
        }
        _ => u64::MAX,
    }
}

pub fn init_syscall() {
    // enable syscall/sysret
    unsafe {
        Efer::update(|flags| {
            flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
        });
    }

    // STAR MSR layout:
    // Bits 63:48 = User CS base for sysretq (CS = this + 16, SS = this + 8)
    // Bits 47:32 = Kernel CS base for syscall (CS = this, SS = this + 8)
    // Bits 31:16 = Ignored in 64-bit mode
    // Bits 15:0  = Ignored in 64-bit mode
    //
    // With GDT:
    //   Index 3 (0x18): User Data (SS for ring 3)
    //   Index 4 (0x20): User Code (CS for ring 3)
    //
    // For sysretq:
    //   CS = (STAR[63:48] + 16) | 3 = 0x20 | 3 = 0x23
    //   SS = (STAR[63:48] + 8) | 3  = 0x18 | 3 = 0x1B
    //
    // STAR[63:48] must be 0x10 (0x20 - 16 = 0x10, 0x18 - 8 = 0x10)
    
    let user_cs_base = 0x10u64;  // STAR[63:48]: sysretq CS = 0x10+16=0x20, SS = 0x10+8=0x18
    let kernel_cs = 0x08u64;      // STAR[47:32]: syscall CS = 0x08 (kernel code)
    // Bits 31:0 are unused in 64-bit mode
    
    let star_value = (user_cs_base << 48) | (kernel_cs << 32);
    
    unsafe {
        msr_write(0xC0000081, star_value);
    }

    // Set LSTAR to syscall handler
    let handler_addr = syscall_handler as *const () as u64;
    unsafe {
        msr_write(0xC0000082, handler_addr);
    }

    // No RFLAGS masking
    unsafe {
        msr_write(0xC0000084, 0u64);
    }

    serial_println_core!("Syscall MSRs initialized");
}