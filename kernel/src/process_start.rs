use alloc::format;
use alloc::string::String;

use crate::process::elf_loader::{PING_ELF, TEST_ELF};
use crate::process::{ElfLoadInfo, process_manager::PROCESS_MANAGER};
use crate::serial_println;

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

    // Launch two ping instances on core 1. They share the same ELF bytes but
    // receive independent address spaces and stacks, so they run as fully
    // separate processes. The scheduler on core 1 runs them round-robin:
    // each calls sys_write a few times then sys_exit, which returns to the
    // scheduler loop and re-enqueues the other.
    if let Ok(pid) = create_userspace_process(&PING_ELF, "ping1", 0, 4) {
        serial_println!("Created ping1 (PID {})", pid);
    }
    if let Ok(pid) = create_userspace_process(&PING_ELF, "ping2", 0, 4) {
        serial_println!("Created ping2 (PID {})", pid);
    }

    serial_println!("Process creation complete");
    serial_println!("All processes are now in the scheduler queues");
}

pub fn create_and_run_init_process() -> ! {
    serial_println!("WARNING: Using deprecated direct execution model");
    serial_println!("Creating init process (old direct execution)");

    let elf_info = match ElfLoadInfo::from_elf_data(&TEST_ELF) {
        Ok(info) => info,
        Err(e) => {
            serial_println!("Failed to parse ELF: {:?}", e);
            panic!("Cannot continue without init process");
        }
    };

    serial_println!("Loaded elf info: {:#x}", elf_info.entry_point);

    let process = match crate::process::Process::create_with_elf(&elf_info, "init", 1, 0) {
        Ok(p) => p,
        Err(e) => {
            serial_println!("Failed to create process: {:?}", e);
            panic!("Cannot create init process");
        }
    };

    serial_println!(
        "Process created: PID={}, name={}",
        process.pid,
        process.name
    );

    serial_println!("About to jump to userspace...");

    crate::process::process::set_current_process(process);

    let curr_process = crate::process::process::get_current_process().lock();
    if let Some(process) = curr_process.as_ref() {
        crate::process::execution::execute_process_direct(process);
    } else {
        serial_println!("Error: No current process set");
        loop {
            x86_64::instructions::hlt();
        }
    }
}
