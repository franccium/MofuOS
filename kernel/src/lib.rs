#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

pub mod allocator;
pub mod gdt;
pub mod graphics;
pub mod interrupts;
pub mod io;
pub mod memory;
pub mod programs;

pub fn init_globals() {
    gdt::init();
    interrupts::init_idt();
}

#[inline(always)]
/// Do nothing loop that tells the CPU to halt until the next interrupt
pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
