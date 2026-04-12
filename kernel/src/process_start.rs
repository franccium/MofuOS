use crate::process::Process;
use crate::process::elf_loader::{ElfLoadInfo, TEST_ELF};
use crate::serial_println;

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
