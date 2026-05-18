/// AP trampoline binary blob
/// This module includes the assembled AP startup trampoline code that runs at physical address 0x8000

#[allow(non_snake_case)]
pub mod ap_trampoline {
    // The binary blob is included at compile time by build.rs
    // It contains the assembled AP trampoline code
    pub const TRAMPOLINE_BINARY: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/ap_trampoline.bin"));

    /// Copy the AP trampoline to physical address 0x8000
    pub unsafe fn copy_to_memory(hhdm_offset: u64) {
        //let dest = (0x8000 + hhdm_offset) as *mut u8;
        let dest = (0x8000) as *mut u8;
        core::ptr::copy_nonoverlapping(TRAMPOLINE_BINARY.as_ptr(), dest, TRAMPOLINE_BINARY.len());
    }

    /// Get the size of the AP trampoline
    pub fn size() -> usize {
        TRAMPOLINE_BINARY.len()
    }
}
