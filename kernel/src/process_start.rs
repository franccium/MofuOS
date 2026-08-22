use alloc::format;
use alloc::string::String;

use crate::process::elf_loader::{
    FS_CACHED_TEST_ELF, FS_TEST_ELF, GAME_ELF, ODYS_ELF, PING_ELF, RUST_FIRST_ELF, SMALL_ELF,
    TEST_ELF, THEOPHE_ELF,
};
use crate::process::{ElfLoadInfo, process_manager::PROCESS_MANAGER};
use crate::{
    RUN_FS_CACHED_TEST, RUN_FS_TEST, RUN_ODYS, RUN_THEOPHE, USE_GAME_PROGRAM, USE_PING_PROGRAM,
    USE_RUST_USER_PROGRAMS, USE_TEST_PROGRAM, serial_println,
};

pub fn create_init_process() {
    serial_println!("Creating init process from ELF");

    let elf_info = match ElfLoadInfo::from_elf_data(&TEST_ELF) {
        Ok(info) => {
            serial_println!("  Parsed ELF: entry_point={:#x}", info.entry_point);
            info
        }
        Err(e) => {
            serial_println!("  ERROR: Failed to parse ELF: {:?}", e);
            return;
        }
    };

    let mut pm = PROCESS_MANAGER.lock();
    match pm.create_process_from_elf(0, &elf_info, "proc1", 5) {
        Ok(init_pid) => {
            serial_println!(
                "Init process created (PID {}), added to scheduler",
                init_pid
            );
        }
        Err(e) => {
            serial_println!("ERROR: Failed to create init process: {:?}", e);
        }
    }
}

pub fn create_userspace_process(
    elf_data: &[u8],
    name: &str,
    parent_pid: usize,
    priority: u8,
) -> Result<usize, String> {
    let elf_info = ElfLoadInfo::from_elf_data(elf_data)
        .map_err(|e| format!("Failed to parse ELF: {:?}", e))?;

    let mut pm = PROCESS_MANAGER.lock();
    let pid = pm
        .create_process_from_elf(parent_pid, &elf_info, name, priority)
        .map_err(|e| format!("Failed to create process: {:?}", e))?;

    serial_println!(
        "Created userspace process '{}' (PID {}) with priority {}",
        name,
        pid,
        priority
    );
    Ok(pid)
}

#[allow(dead_code)]
pub fn create_userspace_processes() {
    serial_println!("Creating initial userspace processes");

    // if let Ok(pid) = create_userspace_process(&RUST_FIRST_ELF, "proc1", 0, 5) {
    //     serial_println!("Created rust_first (PID {})", pid);
    // }

    // if let Ok(pid) = create_userspace_process(&PING_ELF, "ping1", 0, 4) {
    //     serial_println!("Created ping1 (PID {})", pid);
    // }
    // if let Ok(pid) = create_userspace_process(&PING_ELF, "ping2", 0, 4) {
    //     serial_println!("Created ping2 (PID {})", pid);
    // }

    if RUN_THEOPHE {
        if let Ok(pid) = create_userspace_process(&THEOPHE_ELF, "theophe", 0, 7) {
            serial_println!("Created theophe (PID {})", pid);
        }
    }

    if RUN_ODYS {
        if let Ok(pid) = create_userspace_process(&ODYS_ELF, "odys", 0, 7) {
            serial_println!("Created odys (PID {})", pid);
        }
    }

    if RUN_FS_TEST {
        if let Ok(pid) = create_userspace_process(&FS_TEST_ELF, "fs_test", 0, 5) {
            serial_println!("Created fs_test (PID {})", pid);
        }
    }
    if RUN_FS_CACHED_TEST {
        if let Ok(pid) = create_userspace_process(&FS_CACHED_TEST_ELF, "fs_cached_test", 0, 5) {
            serial_println!("Created fs_cached_test (PID {})", pid);
        }
    }

    // if USE_PING_PROGRAM {
    //     if let Ok(pid) = create_userspace_process(&PING_ELF, "ping1", 0, 4) {
    //         serial_println!("Created ping1 (PID {})", pid);
    //     }
    //     if let Ok(pid) = create_userspace_process(&PING_ELF, "ping2", 0, 4) {
    //         serial_println!("Created ping2 (PID {})", pid);
    //     }
    // } else {
    //     if USE_TEST_PROGRAM {
    //         if let Ok(pid) = create_userspace_process(&TEST_ELF, "proc1", 0, 5) {
    //             serial_println!("Created proc1 (PID {})", pid);
    //         }
    //     } else if USE_RUST_USER_PROGRAMS {
    //         if let Ok(pid) = create_userspace_process(&RUST_FIRST_ELF, "proc1", 0, 5) {
    //             serial_println!("Created rust_first (PID {})", pid);
    //         }
    //     } else if USE_GAME_PROGRAM {
    //         if let Ok(pid) = create_userspace_process(&GAME_ELF, "game", 0, 5) {
    //             serial_println!("Created game (PID {})", pid);
    //         }
    //     } else {
    //         if let Ok(pid) = create_userspace_process(&SMALL_ELF, "proc1", 0, 5) {
    //             serial_println!("Created proc1 (PID {})", pid);
    //         }
    //     }
    // }

    serial_println!("Process creation complete");
    serial_println!("All processes are now in the scheduler queues");
}
