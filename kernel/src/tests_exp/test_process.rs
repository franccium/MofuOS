use crate::{process::{
    process_manager::PROCESS_MANAGER,
    syscall::{SystemCall, handle_syscall},
    Process,
    elf_loader::{ElfLoadInfo, TEST_ELF},
}, serial_println};

pub fn test_process_system() {
    {
        let pm = PROCESS_MANAGER.lock();
        if let Ok(arche) = pm.get_process(0) {
            serial_println!("Arche PID: {}, Name: {}", arche.pid, arche.name);
        }
    }

    let process_name = "process_1";
    let process_2_name = "process_2";

    let result1 = handle_syscall(
        0,
        SystemCall::CreateProcess {
            parent_pid: 0,
            name_ptr: process_name.as_ptr(),
            name_len: process_name.len() as u8,
            is_out: false,
        },
    );

    let result2 = handle_syscall(
        0,
        SystemCall::CreateProcess {
            parent_pid: 0,
            name_ptr: process_2_name.as_ptr(),
            name_len: process_2_name.len() as u8,
            is_out: false,
        },
    );

    if result1.is_ok() && result2.is_ok() {
        {
            let pm = PROCESS_MANAGER.lock();
            if let Ok(p1) = pm.get_process(1) {
                serial_println!(
                    "P1 - PID: {}, Name: {}, Parent: {}",
                    p1.pid,
                    p1.name,
                    p1.parent_pid
                );
                serial_println!("P1 - Memory limit: {}", p1.resources.memory_limit);
            }
            if let Ok(p2) = pm.get_process(2) {
                serial_println!(
                    "P2 - PID: {}, Name: {}, Parent: {}",
                    p2.pid,
                    p2.name,
                    p2.parent_pid
                );
                serial_println!("P2 - Memory limit: {}", p2.resources.memory_limit);
            }
        }

        let _ = handle_syscall(
            1,
            SystemCall::TerminateProcess {
                pid_to_kill: 1,
                exit_code: 0,
                kill_children: false,
            },
        );

        {
            let pm = PROCESS_MANAGER.lock();
            if let Ok(p1) = pm.get_process(1) {
                serial_println!("Process 1 after termination:");
                serial_println!("State: {:?}", p1.state);
                serial_println!("Exit code: {:?}", p1.exit_code);
            }
        }
    } else {
        serial_println!("Failed to create processes");
    }
}



pub fn create_init_process() {
    serial_println!("Creating init process");

    let elf_info = match ElfLoadInfo::from_elf_data(&TEST_ELF) {
        Ok(info) => info,
        Err(e) => {
            serial_println!("Failed to parse ELF: {:?}", e);
            return;
        }
    };
    serial_println!("Loaded elf info: {:#x}", elf_info.entry_point);

    let process = match Process::create_with_elf(&elf_info, "init", 1, 0) {
        Ok(p) => p,
        Err(e) => {
            serial_println!("Failed to create process: {:?}", e);
            return;
        }
    };

    serial_println!(
        "Process created: PID={}, name={}",
        process.pid,
        process.name
    );

    // {
    //     let mut pm = PROCESS_MANAGER.lock();
    //     pm.add_process(process.clone());
    // }

    //SCHEDULER.add_process(process.pid, process.priority);

    serial_println!("Init process added to scheduler");
}

pub fn create_and_run_init_process() -> ! {
    serial_println!("Creating init process");

    let elf_info = match ElfLoadInfo::from_elf_data(TEST_ELF) {
        Ok(info) => info,
        Err(e) => {
            serial_println!("Failed to parse ELF: {:?}", e);
            panic!("Cannot continue without init process");
        }
    };

    serial_println!("Loaded elf info: {:#x}", elf_info.entry_point);

    let process = match Process::create_with_elf(&elf_info, "init", 1, 0) {
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
