#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(allocator_api)]
#![feature(portable_simd)]
#![allow(warnings, unused)] // TODO: remove this

use static_assertions::const_assert;

pub const HHDM_OFFSET: u64 = 0xFFFF_8000_0000_0000;
pub const MAX_CORES: u8 = 16;
/// Number of Application Processor (AP) cores — all cores except the BSP (core 0)
pub const AP_CORE_COUNT: u8 = MAX_CORES - 1;
const_assert!(MAX_CORES <= 64); // NOTE: Hard limit to 64 cores for the core pool availability 64-bit long bitmap

pub mod memory;
extern crate alloc;
pub mod asm;
pub mod boot_info;
pub mod data_structures;
pub mod filesystem;
pub mod gdt;
pub mod graphics;
pub mod interrupts;
pub mod io;
pub mod process;
pub mod process_start;
pub mod programs;
pub mod tests_exp;
pub mod util;

pub use alloc::string::String;

extern crate lazy_static;

#[inline(always)]
/// Do nothing loop that tells the CPU to halt until the next interrupt
pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
