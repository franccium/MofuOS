use crate::process::{
    process::INVALID_PID,
    process_manager::{ARCHE_PID, PROCESS_MANAGER},
};
use crate::serial_println;

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
            match pm.create_process(parent_pid, priority, name_ptr, name_len, is_out) {
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
