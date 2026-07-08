use crate::{hlt_loop, process::Process, serial_println, serial_println_core};
use core::arch::asm;
use x86_64::{
    PhysAddr,
    registers::control::{Cr3, Cr3Flags},
    structures::paging::PhysFrame,
};

pub fn execute_process_direct(process: &Process) -> ! {
    serial_println!("execute_process_direct");
    serial_println!("PID: {}", process.pid);
    serial_println!("Entry point: {:#x}", process.execution_context.rip);
    serial_println!("Stack pointer: {:#x}", process.execution_context.rsp);
    serial_println!(
        "Page table: {:#x}",
        process.execution_context.page_table_base_phys
    );

    // switch to the process's page table
    let page_table_frame = PhysFrame::containing_address(PhysAddr::new(
        process.execution_context.page_table_base_phys,
    ));
    unsafe {
        Cr3::write(page_table_frame, Cr3Flags::empty());
    }

    serial_println!("Switched to user page table");

    unsafe {
        jump_to_userspace(
            process.execution_context.rip,
            process.execution_context.rsp,
            process.execution_context.rflags,
            0,
        );
    }
}

#[unsafe(no_mangle)]
pub unsafe fn jump_to_userspace(entry_point: u64, stack_pointer: u64, rflags: u64, core_id: u8) -> ! {
    let user_code_selector = crate::gdt::get_user_code_selector(core_id).0 as u64;
    let user_data_selector = crate::gdt::get_user_data_selector(core_id).0 as u64;
    serial_println_core!(
        "jump_to_userspace: entry_point={:#x}, stack_pointer={:#x}, rflags={:#x}, core_id={}, user_code_selector={:#x}, user_data_selector={:#x}",
        entry_point,
        stack_pointer,
        rflags,
        core_id,
        user_code_selector,
        user_data_selector
    );

    x86_64::instructions::interrupts::disable();

    unsafe {
        asm!(
            "mov ds, {data_sel:x}",
            "mov es, {data_sel:x}",
            "mov fs, {data_sel:x}",
            "mov gs, {data_sel:x}",

            "push {data_sel}",   // SS
            "push {stack_ptr}",  // RSP
            "push {rflags}",     // RFLAGS
            "push {code_sel}",   // CS
            "push {entry}",      // RIP

            "iretq",

            data_sel = in(reg) user_data_selector,
            stack_ptr = in(reg) stack_pointer,
            rflags = in(reg) rflags,
            code_sel = in(reg) user_code_selector,
            entry = in(reg) entry_point,
            options(noreturn)
        );
    }
    hlt_loop()
}
