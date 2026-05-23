#[allow(non_snake_case)]
pub mod ap_trampoline {
    pub const TRAMPOLINE_BINARY: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/ap_trampoline.bin"));

    pub unsafe fn copy_to_memory(hhdm_offset: u64) {
        let dest = 0x8000 as *mut u8;
        core::ptr::copy_nonoverlapping(TRAMPOLINE_BINARY.as_ptr(), dest, TRAMPOLINE_BINARY.len());
    }

    pub fn size() -> usize {
        TRAMPOLINE_BINARY.len()
    }
}
