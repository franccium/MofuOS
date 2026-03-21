#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

pub mod io;
pub mod memory;

pub fn init() {
}

#[inline(always)]
/// Do nothing loop that tells the CPU to halt until the next interrupt
pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}