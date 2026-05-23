use crate::asm::ap_trampoline;
use crate::interrupts::{
    self, get_lapic_base_addr, init_timer_for_core, map_local_apic_for_current_core,
};
use crate::process::{CORE_POOL, SCHEDULER};
use crate::serial_println;
use crate::util::apic::APICOffset;
use crate::util::msr::msr_read;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::arch::asm;
use limine::mp::Cpu;
use spin::Once;
use x86_64::instructions::hlt;
use x86_64::structures::paging::{FrameAllocator, Mapper, Size4KiB};

static mut CPU_INFO: CpuInfo = CpuInfo {
    features: CpuFeatureFlags::empty(),
    cache_line_size: 0,
    apic_id: 0,
    family: 0,
    model: 0,
    stepping: 0,
    vendor: CpuVendor::Unknown,
};

static CPU_INFO_PER_CORE: Once<Vec<CpuInfo>> = Once::new();

pub struct CpuInfo {
    pub features: CpuFeatureFlags,
    pub cache_line_size: u8,
    pub apic_id: u8,

    pub family: u8,
    pub model: u8,
    pub stepping: u8,
    pub vendor: CpuVendor,
}

bitflags! {
    pub struct CpuFeatureFlags: u32 {
        const APIC = 1 << 0;
        const X2APIC = 1 << 1;
        const TSC = 1 << 2; // Time Stamp Counter
        const TSC_DEADLINE = 1 << 3; // TSC Deadline mode
        const PGE = 1 << 4; // Page Global Enable
        const PAT = 1 << 5; // Page Attribute Table
        const SSE = 1 << 6;
        const SSE2 = 1 << 7;
        const SSE3 = 1 << 8;
        const SSE4_1 = 1 << 9;
        const SSE4_2 = 1 << 10;
        const AVX = 1 << 11;
        const AES = 1 << 12;
        const RDRAND = 1 << 13;
        const HYPERVISOR = 1 << 14; // indicates running inside a VM
    }
}

pub enum CpuVendor {
    Intel,
    Amd,
    Unknown,
}

impl core::fmt::Display for CpuVendor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CpuVendor::Intel => write!(f, "Intel"),
            CpuVendor::Amd => write!(f, "AMD"),
            CpuVendor::Unknown => write!(f, "Unknown"),
        }
    }
}

unsafe fn get_vendor(ebx: u32, ecx: u32, edx: u32) -> CpuVendor {
    let vendor_bytes: [u8; 12] = [
        (ebx >> 0) as u8,
        (ebx >> 8) as u8,
        (ebx >> 16) as u8,
        (ebx >> 24) as u8,
        (edx >> 0) as u8,
        (edx >> 8) as u8,
        (edx >> 16) as u8,
        (edx >> 24) as u8,
        (ecx >> 0) as u8,
        (ecx >> 8) as u8,
        (ecx >> 16) as u8,
        (ecx >> 24) as u8,
    ];

    if &vendor_bytes == b"GenuineIntel" {
        CpuVendor::Intel
    } else if &vendor_bytes == b"AuthenticAMD" {
        CpuVendor::Amd
    } else {
        CpuVendor::Unknown
    }
}

unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;

    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx:e}, ebx",
            "pop rbx",
            ebx = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") 0 => ecx,
            out("edx") edx,
            options(nostack, preserves_flags)
        );
    }

    (eax, ebx, ecx, edx)
}

pub unsafe fn init_cpu_info() {
    let (max_leaf, vendor_ebx, vendor_ecx, vendor_edx) = unsafe { cpuid(0) };
    serial_println!(
        "init_cpu_info: read leaf 0: eax: {:#x}, ebx: {:#x}, ecx: {:#x}, edx: {:#x}",
        max_leaf,
        vendor_ebx,
        vendor_ecx,
        vendor_edx
    );

    assert!(max_leaf >= 1);
    let (_, feat_ebx, feat_ecx, feat_edx) = unsafe { cpuid(1) };
    serial_println!(
        "init_cpu_info: read leaf 1: ebx: {:#x}, ecx: {:#x}, edx: {:#x}",
        feat_ebx,
        feat_ecx,
        feat_edx
    );

    let mut features = CpuFeatureFlags::empty();

    if feat_edx & (1 << 9) != 0 {
        features |= CpuFeatureFlags::APIC;
    }
    if feat_ecx & (1 << 21) != 0 {
        features |= CpuFeatureFlags::X2APIC;
    }
    if feat_edx & (1 << 4) != 0 {
        features |= CpuFeatureFlags::TSC;
    }
    if feat_ecx & (1 << 24) != 0 {
        features |= CpuFeatureFlags::TSC_DEADLINE;
    }
    if feat_edx & (1 << 13) != 0 {
        features |= CpuFeatureFlags::PGE;
    }
    if feat_edx & (1 << 16) != 0 {
        features |= CpuFeatureFlags::PAT;
    }
    if feat_edx & (1 << 25) != 0 {
        features |= CpuFeatureFlags::SSE;
    }
    if feat_edx & (1 << 26) != 0 {
        features |= CpuFeatureFlags::SSE2;
    }
    if feat_ecx & (1 << 0) != 0 {
        features |= CpuFeatureFlags::SSE3;
    }
    if feat_ecx & (1 << 19) != 0 {
        features |= CpuFeatureFlags::SSE4_1;
    }
    if feat_ecx & (1 << 20) != 0 {
        features |= CpuFeatureFlags::SSE4_2;
    }
    if feat_ecx & (1 << 28) != 0 {
        features |= CpuFeatureFlags::AVX;
    }
    if feat_ecx & (1 << 25) != 0 {
        features |= CpuFeatureFlags::AES;
    }
    if feat_ecx & (1 << 30) != 0 {
        features |= CpuFeatureFlags::RDRAND;
    }
    if feat_ecx & (1 << 31) != 0 {
        features |= CpuFeatureFlags::HYPERVISOR;
    }

    let cache_line_size = ((feat_ebx >> 8) & 0xFF) as u8 * 8;
    let apic_id = ((feat_ebx >> 24) & 0xFF) as u8;
    let cpu_family = ((feat_edx >> 8) & 0xF) as u8;
    let cpu_model = ((feat_edx >> 4) & 0xF) as u8;
    let cpu_stepping = (feat_edx & 0xF) as u8;
    let cpu_vendor = unsafe { get_vendor(vendor_ebx, vendor_ecx, vendor_edx) };
    serial_println!("CPU Info:");
    serial_println!("  Vendor: {}", cpu_vendor);
    serial_println!("  Cache Line Size: {}", cache_line_size);
    serial_println!("  Features {:#b}", features);

    unsafe {
        CPU_INFO = CpuInfo {
            features,
            cache_line_size,
            apic_id,
            family: cpu_family,
            model: cpu_model,
            stepping: cpu_stepping,
            vendor: cpu_vendor,
        };
    }
}

pub fn init_cpu_infos(cpus: &[&Cpu]) {
    let core_count = cpus.len();
    serial_println!("init_cpu_infos: {} cores", core_count);

    let mut vec = Vec::with_capacity(core_count);
    for _ in 0..core_count {
        let default = CpuInfo {
            features: CpuFeatureFlags::empty(),
            cache_line_size: 0,
            apic_id: 0,
            family: 0,
            model: 0,
            stepping: 0,
            vendor: CpuVendor::Unknown,
        };
        vec.push(default);
    }

    CPU_INFO_PER_CORE.call_once(|| vec);
}

pub unsafe fn init_current_core() {
    let core_id = get_current_core_id();

    serial_println!("Core {}: Initializing", core_id);

    unsafe { init_cpu_info_for_core(core_id) };
    unsafe { init_timer_for_core(core_id) };

    serial_println!("Core {}: Initialized successfully", core_id);
}

pub unsafe fn init_cpu_info_for_core(core_id: u8) {
    let (max_leaf, vendor_ebx, vendor_ecx, vendor_edx) = unsafe { cpuid(0) };
    serial_println!(
        "Core{}: init_cpu_info: read leaf 0: eax: {:#x}, ebx: {:#x}, ecx: {:#x}, edx: {:#x}",
        core_id,
        max_leaf,
        vendor_ebx,
        vendor_ecx,
        vendor_edx
    );

    assert!(max_leaf >= 1);
    let (_, feat_ebx, feat_ecx, feat_edx) = unsafe { cpuid(1) };
    serial_println!(
        "Core{}: init_cpu_info: read leaf 1: ebx: {:#x}, ecx: {:#x}, edx: {:#x}",
        core_id,
        feat_ebx,
        feat_ecx,
        feat_edx
    );

    let mut features = CpuFeatureFlags::empty();

    if feat_edx & (1 << 9) != 0 {
        features |= CpuFeatureFlags::APIC;
    }
    if feat_ecx & (1 << 21) != 0 {
        features |= CpuFeatureFlags::X2APIC;
    }
    if feat_edx & (1 << 4) != 0 {
        features |= CpuFeatureFlags::TSC;
    }
    if feat_ecx & (1 << 24) != 0 {
        features |= CpuFeatureFlags::TSC_DEADLINE;
    }
    if feat_edx & (1 << 13) != 0 {
        features |= CpuFeatureFlags::PGE;
    }
    if feat_edx & (1 << 16) != 0 {
        features |= CpuFeatureFlags::PAT;
    }
    if feat_edx & (1 << 25) != 0 {
        features |= CpuFeatureFlags::SSE;
    }
    if feat_edx & (1 << 26) != 0 {
        features |= CpuFeatureFlags::SSE2;
    }
    if feat_ecx & (1 << 0) != 0 {
        features |= CpuFeatureFlags::SSE3;
    }
    if feat_ecx & (1 << 19) != 0 {
        features |= CpuFeatureFlags::SSE4_1;
    }
    if feat_ecx & (1 << 20) != 0 {
        features |= CpuFeatureFlags::SSE4_2;
    }
    if feat_ecx & (1 << 28) != 0 {
        features |= CpuFeatureFlags::AVX;
    }
    if feat_ecx & (1 << 25) != 0 {
        features |= CpuFeatureFlags::AES;
    }
    if feat_ecx & (1 << 30) != 0 {
        features |= CpuFeatureFlags::RDRAND;
    }
    if feat_ecx & (1 << 31) != 0 {
        features |= CpuFeatureFlags::HYPERVISOR;
    }

    let cache_line_size = ((feat_ebx >> 8) & 0xFF) as u8 * 8;
    let apic_id = ((feat_ebx >> 24) & 0xFF) as u8;
    let cpu_family = ((feat_edx >> 8) & 0xF) as u8;
    let cpu_model = ((feat_edx >> 4) & 0xF) as u8;
    let cpu_stepping = (feat_edx & 0xF) as u8;
    let cpu_vendor = unsafe { get_vendor(vendor_ebx, vendor_ecx, vendor_edx) };
    serial_println!("Core{}: CPU Info:", core_id);
    serial_println!("  Vendor: {}", cpu_vendor);
    serial_println!("  Cache Line Size: {}", cache_line_size);
    serial_println!("  Features {:#b}", features);

    unsafe {
        let mut cpu_infos = CPU_INFO_PER_CORE.get().unwrap();
        let ptr = cpu_infos.as_ptr() as *mut CpuInfo;
        ptr.add(core_id as usize).write(CpuInfo {
            features,
            cache_line_size,
            apic_id,
            family: cpu_family,
            model: cpu_model,
            stepping: cpu_stepping,
            vendor: cpu_vendor,
        });
    }
}

pub fn get_cpu_info() -> &'static CpuInfo {
    unsafe {
        let ptr = &raw const CPU_INFO;
        &*ptr
    }
}

pub fn get_cpu_info_for_core(core_id: u8) -> &'static CpuInfo {
    unsafe {
        let ptr = &raw const CPU_INFO_PER_CORE.get().unwrap()[core_id as usize];
        &*ptr
    }
}

// pub fn get_lapic_base_addr() -> *mut u32 {
//     const IA32_APIC_BASE_MSR: u32 = 0x1B;
//     unsafe {
//         let low: u32;
//         let high: u32;
//         asm!(
//             "rdmsr",
//             in("ecx") IA32_APIC_BASE_MSR,
//             out("eax") low,
//             out("edx") high,
//             options(nostack, preserves_flags)
//         );
//         let apic_base = ((high as u64) << 32) | (low as u64);

//         let base_addr = apic_base & 0xFFFFF000;
//         base_addr as *mut u32
//     }
// }

//TODO: For now assume APIC ID = core index
pub fn get_current_core_id() -> u8 {
    unsafe {
        let lapic = get_lapic_base_addr();
        let apic_id_reg = lapic.offset(APICOffset::IDr as isize / 4);
        let apic_id = (apic_id_reg.read_volatile() >> 24) as u8;

        apic_id
    }
}

pub fn get_current_cpu_info() -> &'static CpuInfo {
    let core_id = get_current_core_id();
    get_cpu_info_for_core(core_id)
}

unsafe extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
}

/// Start an AP core using INIT-SIPI-SIPI sequence
const TRAMPOLINE_PHYS: u64 = 0x8000;
const MAGIC_OFFSET: u64 = 0x8FF0; // AP writes "APST" here
pub const SECRET_MESSAGE_OFFSET: u64 = 0x8F30; // AP writes "APST" here
pub const AP_CORE_CR3_MESSAGE_OFFSET: u64 = 0x8F40;
pub const AP_CORE_APIC_ID_MESSAGE_OFFSET: u64 = 0x8F48;
pub const AP_CORE_FUNCTION_ACHIEVED: u64 = 0x1234CCCC;

const ACK_OFFSET: u64 = MAGIC_OFFSET; // BSP writes 1 here
const DONE_OFFSET: u64 = MAGIC_OFFSET; // AP writes 1 when leaving trampoline
const GDT_OFFSET: u64 = 0x8FF8; // GDT descriptor
const CR3_OFFSET: u64 = 0x9000;
const STACK_OFFSET: u64 = 0x9008;
const ENTRY_OFFSET: u64 = 0x9010;

const WRITTEN_CR3_OFFSET: u64 = 0x9020;

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;

/// Manages stack allocation for AP cores
pub struct ApStackAllocator {
    /// Base virtual address for AP stacks
    stack_base: VirtAddr,
    /// Size of each AP stack (typically 64KB or 128KB)
    stack_size: u64,
    /// Number of stacks allocated so far
    next_stack: AtomicU64,
    /// Maximum number of AP cores supported
    max_aps: u32,
}

/// Represents an allocated stack for an AP core
#[derive(Debug)]
pub struct ApStack {
    /// Virtual address of the stack top (highest address, stack grows down)
    top: VirtAddr,
    /// Virtual address of the stack bottom (lowest address)
    bottom: VirtAddr,
    /// Size of the stack in bytes
    size: u64,
}

impl ApStack {
    /// Get the stack pointer value (top of stack for x86)
    pub fn top(&self) -> u64 {
        self.top.as_u64()
    }

    /// Get the bottom of stack address
    pub fn bottom(&self) -> u64 {
        self.bottom.as_u64()
    }

    /// Get the stack size
    pub fn size(&self) -> u64 {
        self.size
    }
}

// Global stack allocator
static AP_STACK_ALLOCATOR: spin::Once<ApStackAllocator> = spin::Once::new();

impl ApStackAllocator {
    /// Initialize the AP stack allocator
    pub fn init(stack_base: VirtAddr, stack_size: u64, max_aps: u32) {
        AP_STACK_ALLOCATOR.call_once(|| ApStackAllocator {
            stack_base,
            stack_size,
            next_stack: AtomicU64::new(0),
            max_aps,
        });
    }

    /// Get the global allocator instance
    pub fn get() -> &'static ApStackAllocator {
        AP_STACK_ALLOCATOR
            .get()
            .expect("AP stack allocator not initialized")
    }

    /// Allocate a new stack for an AP core
    pub fn allocate_stack(&self, core_id: u8) -> Option<ApStack> {
        let stack_index = self.next_stack.fetch_add(1, Ordering::SeqCst);

        if stack_index >= self.max_aps as u64 {
            return None;
        }

        let guard_pages = 1;
        let total_space = self.stack_size + (guard_pages * 0x1000);
        let stack_offset = stack_index * total_space;

        // The stack region looks like this in memory:
        // High address:  [stack_top]     <- RSP starts here
        //                [stack grows downward]
        // Low address:   [stack_bottom]
        //                [guard page]

        let stack_top = self.stack_base + stack_offset + self.stack_size;
        let stack_bottom = self.stack_base + stack_offset;

        // Align stack top to 16 bytes for ABI compatibility
        let aligned_top = stack_top.align_down(16u64);

        Some(ApStack {
            top: aligned_top,     // RSP starts here (high address)
            bottom: stack_bottom, // Stack ends here (low address)
            size: self.stack_size,
        })
    }
}

/// Allocate a stack for an AP core (convenience function)
pub fn allocate_ap_stack(core_id: u8) -> ApStack {
    let allocator = ApStackAllocator::get();

    allocator
        .allocate_stack(core_id)
        .unwrap_or_else(|| panic!("Failed to allocate stack for AP core {}", core_id))
}

const AP_STACK_SIZE: u64 = 8 * 1024; // 16KB per stack
const MAX_AP_CORES: u32 = 2; // Support up to 16 AP cores

// Virtual address where AP stacks will be mapped
// Make sure this doesn't conflict with your kernel's memory layout!
const AP_STACK_BASE: u64 = 0xFFFF_F40F_0000_0000; // Example address

pub fn init_ap_support(
    page_table: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) {
    // let stack_base = VirtAddr::new(0x80000000);

    //     // Initialize and map
    //     initialize_ap_stack_memory(
    //         page_table,
    //         frame_allocator,
    //         stack_base,
    //         128 * 1024,
    //         4,
    //     ).expect("Failed to map AP stack memory");

    //     // CRITICAL: Verify the mapping works from the BSP
    //     let test_addr = 0x80020000 as *const u64;
    //     unsafe {
    //         match core::ptr::read_volatile(test_addr) {
    //             val => serial_println!("AP stack test read: {:#x}", val),
    //         }
    //     }
    //     serial_println!("AP stack mapping verified!");

    // Initialize stack allocator
    ApStackAllocator::init(VirtAddr::new(AP_STACK_BASE), AP_STACK_SIZE, MAX_AP_CORES);

    // Map memory for AP stacks in page tables
    initialize_ap_stack_memory(
        page_table,
        frame_allocator,
        VirtAddr::new(AP_STACK_BASE),
        AP_STACK_SIZE,
        MAX_AP_CORES,
    )
    .expect("Failed to map AP stack memory");

    serial_println!("AP stack allocator initialized:");
    serial_println!("  Base: {:#x}", AP_STACK_BASE);
    serial_println!("  Stack size: {} KB", AP_STACK_SIZE / 1024);
    serial_println!("  Max APs: {}", MAX_AP_CORES);

    unsafe {
        use x86_64::registers::control::Cr3;
        let (frame, flags) = Cr3::read();
        Cr3::write(frame, flags);
    }
}

/// Initialize AP stack memory region in page tables
pub fn initialize_ap_stack_memory(
    page_table: &mut impl x86_64::structures::paging::Mapper<Size4KiB>,
    frame_allocator: &mut impl x86_64::structures::paging::FrameAllocator<Size4KiB>,
    stack_base: VirtAddr,
    stack_size: u64,
    max_aps: u32,
) -> Result<(), x86_64::structures::paging::mapper::MapToError<Size4KiB>> {
    use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};

    let total_space = (stack_size + 0x1000) * max_aps as u64; // + guard pages
    let num_pages = (total_space + 0xFFF) / 0x1000; // Round up

    for i in 0..num_pages {
        let page = Page::<Size4KiB>::from_start_address(stack_base + (i * 0x1000))
            .expect("Invalid page address");

        // Allocate physical frame for this page
        let frame = frame_allocator
            .allocate_frame()
            .expect("Failed to allocate frame for AP stack");

        unsafe {
            page_table
                .map_to(
                    page,
                    frame,
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
                    frame_allocator,
                )?
                .flush();
        }

        serial_println!(
            "Mapped AP stack page: virtual {:#x} -> physical {:#x}",
            page.start_address(),
            frame.start_address()
        );
    }

    Ok(())
}
// Test self-IPI (should just interrupt ourselves)
unsafe fn test_lapic_ipi() {
    let lapic = get_lapic_base_addr();
    let icr_low = lapic.offset(0x300 / 4);
    let icr_high = lapic.offset(0x310 / 4);

    serial_println!("Testing LAPIC IPI to self...");

    // Wait for idle
    while (icr_low.read_volatile() & (1 << 12)) != 0 {
        core::hint::spin_loop();
    }

    // Send a fixed IPI to self (vector 0x30 for example)
    // Shorthand 01 = Self
    let self_ipi = 0x00004030; // Shorthand=self, Fixed delivery, vector=0x30
    icr_low.write_volatile(self_ipi);

    serial_println!("Self IPI sent (should trigger interrupt vector 0x30)");

    // Wait a bit
    for _ in 0..10000 {
        core::hint::spin_loop();
    }

    serial_println!("Self IPI test complete");

    // Check if x2APIC is enabled
    let apic_base = unsafe { msr_read(0x1B) }; // IA32_APIC_BASE MSR
    let x2apic_enabled = (apic_base >> 10) & 1 == 1; // Bit 10 is x2APIC enable
    serial_println!("x2APIC enabled: {}", x2apic_enabled);

    // Check number of local APICs
    let lapic_id_reg = lapic.offset(0x20 / 4);
    let my_id = (lapic_id_reg.read_volatile() >> 24) as u8;

    // Check LAPIC version register to see max LAPIC ID
    let version_reg = lapic.offset(0x30 / 4);
    let version = version_reg.read_volatile();
    let max_lvt = (version >> 16) & 0xFF;
    serial_println!("LAPIC version: {:#x}", version);
    serial_println!("Max LVT entry: {}", max_lvt);

    // Try sending SIPI to APIC ID 255 (should not exist)
    serial_println!("Testing SIPI to non-existent APIC 255...");
    while (icr_low.read_volatile() & (1 << 12)) != 0 {
        core::hint::spin_loop();
    }
    icr_high.write_volatile((255u32) << 24);
    icr_low.write_volatile(0x00004608);
    serial_println!("SIPI to APIC 255 sent! (if we get here, sending itself works)");
}
pub unsafe fn start_ap_core(
    core_id: u8,
    apic_id: u8,
    hhdm_offset: u64,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    use x86_64::registers::control::Cr3;

    serial_println!("Starting AP core {} with APIC ID {}", core_id, apic_id);

    test_lapic_ipi();

    for addr in (0x8F10..0x8FFFF).step_by(2) {
        let ptr = (addr + hhdm_offset) as *mut u16;
        core::ptr::write_volatile(ptr, 0);
    }

    // Allocate a stack for this AP
    let ap_stack = allocate_ap_stack(core_id); // You need to implement this

    serial_println!(
        "Allocated stack for AP core {}: top={:#x}, bottom={:#x}, size={} KB",
        core_id,
        ap_stack.top(),
        ap_stack.bottom(),
        ap_stack.size() / 1024
    );

    serial_println!("Core {}: Copying AP trampoline to memory", core_id);
    ap_trampoline::copy_to_memory(hhdm_offset);

    let phys_byte = core::ptr::read_volatile((0x8000 + hhdm_offset) as *const u8);
    let identity_byte = core::ptr::read_volatile(0x8000 as *const u8);
    serial_println!("Physical 0x8000: {:#x}", phys_byte);
    serial_println!("Identity 0x8000: {:#x}", identity_byte);
    serial_println!("Expected (first byte of trampoline): 0xFA (CLI)");

    // Clear synchronization flags
    let magic_ptr = (MAGIC_OFFSET + hhdm_offset) as *mut u32;
    let ack_ptr = (ACK_OFFSET + hhdm_offset) as *mut u8;
    let done_ptr = (DONE_OFFSET + hhdm_offset) as *mut u8;

    core::ptr::write_volatile(magic_ptr, 0);
    core::ptr::write_volatile(ack_ptr, 0);
    core::ptr::write_volatile(done_ptr, 0);

    // Write GDT descriptor to trampoline
    let gdt_addr: u32 = (GDT_OFFSET + hhdm_offset) as u32; // Physical GDT address
    let gdtr_ptr = (GDT_OFFSET + hhdm_offset) as *mut u64;
    // GDT is at 0x80F0, so write descriptor: limit = 23, base = 0x80F0
    core::ptr::write_volatile(gdtr_ptr, 0x80F0_0017_0000); // base:16 | limit:16

    // Get CR3
    let (l4_table_frame, _flags) = Cr3::read();
    let cr3_phys = l4_table_frame.start_address().as_u64();

    // Write CR3 and stack and entry point
    let cr3_ptr = (CR3_OFFSET + hhdm_offset) as *mut u64;
    let stack_ptr = (STACK_OFFSET + hhdm_offset) as *mut u64;
    let entry_ptr = (ENTRY_OFFSET + hhdm_offset) as *mut u64;
    let cr3_written_ptr = (WRITTEN_CR3_OFFSET + hhdm_offset) as *mut u64;

    let trampoline_written_cr3 = core::ptr::write_volatile(0x9060 as *mut u64, 0x1111111111111111);

    // let stack_ptr = 0x9008 as *mut u64;  // Identity-mapped, no HHDM offset!

    // // Hardcode the stack to  ap_stack.top() (physical, identity-mapped)
    // let temp_stack: u64 =  ap_stack.top();
    // core::ptr::write_volatile(stack_ptr, temp_stack);

    // // VERIFY
    // let verify = core::ptr::read_volatile(stack_ptr);
    // serial_println!("Stack value at 0x9008: {:#x}", verify);

    // if verify !=  ap_stack.top() {
    //     serial_println!("FATAL: Cannot write stack pointer!");
    //     return Err("Stack write failed");
    // }

    // Make sure  ap_stack.top() is accessible
    let stack_test = ap_stack.bottom() as *mut u64;
    core::ptr::write_volatile(stack_test, 0xCAFEBABE_DEADBEEFu64);
    let stack_verify = core::ptr::read_volatile(stack_test);

    if stack_verify != 0xCAFEBABE_DEADBEEFu64 {
        serial_println!("FATAL: Stack at  ap_stack.bottom() not writable!");
        serial_println!("Need to identity-map  ap_stack.bottom() first!");
        return Err("Stack not mapped");
    }

    serial_println!("Stack at  ap_stack.bottom() verified writable!");

    // In start_ap_core, after writing stack_ptr:

    serial_println!("Writing CR3 for AP core: {:#x}", cr3_phys);

    core::ptr::write_volatile(cr3_ptr, cr3_phys);
    //core::ptr::write_volatile(stack_ptr, ap_stack.top());
    core::ptr::write_volatile(stack_ptr, ap_stack.top());
    core::ptr::write_volatile(entry_ptr, ap_core_entry_point as u64);

    core::ptr::write_volatile(cr3_written_ptr, 1);

    let stack_value = core::ptr::read_volatile(stack_ptr);
    serial_println!("Stack pointer value at 0x9008: {:#x}", stack_value);

    // Try to read from that stack address on the BSP
    let test_stack_ptr = stack_value as *const u64;
    serial_println!("Attempting to read from stack address...");
    // This will page fault on BSP if the stack isn't mapped!
    let test_read = core::ptr::read_volatile(test_stack_ptr);
    serial_println!("Successfully read from stack: {:#x}", test_read);

    // // After setting up identity mapping, check if it's in the PML4 you're sharing:
    // let (pml4_frame, _) = Cr3::read();
    // let pml4_phys = pml4_frame.start_address().as_u64();
    // let pml4_virt_ptr = (pml4_phys + hhdm_offset) as *const x86_64::structures::paging::page_table::PageTable;

    // // Read the PML4 entry for  ap_stack.top() (index = ( ap_stack.top() >> 39) & 0x1FF = 0)
    // let pml4_index = ( ap_stack.top() >> 39) & 0x1FF;
    // let pml4_entry = unsafe { &(pml4_virt_ptr)[pml4_index] };
    // serial_println!("PML4[{}] for  ap_stack.top(): {:#x}", pml4_index, pml4_entry.addr().as_u64());

    // if pml4_entry.is_unused() {
    //     serial_println!("CRITICAL:  ap_stack.top() NOT in page tables! PML4 entry is empty!");
    // }

    let (active_pml4_frame, _) = Cr3::read();
    serial_println!(
        "3 Active PML4 frame: {:#x}",
        active_pml4_frame.start_address().as_u64()
    );
    // === DEBUG: Check LAPIC access ===
    serial_println!("=== LAPIC Debug ===");

    let lapic = get_lapic_base_addr();
    serial_println!("LAPIC base pointer: {:p}", lapic);

    let lapic_id = unsafe {
        let id_reg = lapic.offset(0x20 / 4);
        (id_reg.read_volatile() >> 24) as u8
    };

    serial_println!("=== APIC MSR Debug ===");
    let apic_base = unsafe { msr_read(0x1B) };
    serial_println!("IA32_APIC_BASE MSR: {:#018x}", apic_base);
    serial_println!("  Physical base: {:#x}", apic_base & 0xFFFFF000);
    serial_println!("  BSP (bit 8): {}", (apic_base >> 8) & 1);
    serial_println!("  x2APIC (bit 10): {}", (apic_base >> 10) & 1);
    serial_println!("  APIC Enable (bit 11): {}", (apic_base >> 11) & 1);

    if (apic_base >> 11) & 1 == 0 {
        serial_println!("CRITICAL: APIC is DISABLED in MSR!");
        serial_println!("Enabling it now...");

        // Enable APIC by setting bit 11
        let new_base = apic_base | (1 << 11);
        unsafe {
            let low = new_base as u32;
            let high = (new_base >> 32) as u32;
            asm!(
                "wrmsr",
                in("ecx") 0x1Bu32,
                in("eax") low,
                in("edx") high,
                options(nostack, preserves_flags)
            );
        }

        // Re-read to verify
        let verify = unsafe { msr_read(0x1B) };
        serial_println!("After enable: {:#018x}", verify);
        serial_println!("APIC enabled: {}", (verify >> 11) & 1 == 1);
    }

    serial_println!("[BSP] Current LAPIC ID: {}", lapic_id);

    // Try reading LAPIC ID register (offset 0x20)
    let lapic_id_reg = lapic.offset(0x20 / 4);
    serial_println!("LAPIC ID reg pointer: {:p}", lapic_id_reg);

    // Read LAPIC ID - if this crashes, LAPIC isn't mapped properly
    let lapic_id_val = lapic_id_reg.read_volatile();
    serial_println!("LAPIC ID value: {:#x}", lapic_id_val);

    // Check ICR registers
    let icr_low = lapic.offset(0x300 / 4);
    let icr_high = lapic.offset(0x310 / 4);
    serial_println!("ICR low pointer: {:p}", icr_low);
    serial_println!("ICR high pointer: {:p}", icr_high);

    // Read ICR low to check delivery status
    let icr_low_val = icr_low.read_volatile();
    serial_println!("ICR low initial value: {:#x}", icr_low_val);
    serial_println!("Delivery status: {}", (icr_low_val >> 12) & 1);

    // === Try sending INIT ===
    serial_println!("Core {}: Sending INIT IPI to APIC ID {}", core_id, apic_id);

    // Wait for delivery status to clear
    while (icr_low.read_volatile() & (1 << 12)) != 0 {
        core::hint::spin_loop();
    }
    serial_println!("ICR ready for INIT");

    // Set destination
    icr_high.write_volatile((apic_id as u32) << 24);
    serial_println!("Set destination to APIC ID {}", apic_id);

    serial_println!("Core {}: Sending INIT IPI to APIC ID {}", core_id, apic_id);
    // INIT IPI: Delivery Mode=101 (INIT), Physical, Level=Assert
    // Vector = 0 (INIT ignores vector)
    // 0x4500 = 0b0100_0101_0000_0000
    //              ^^   ^ ^           Level=Assert (1), Trigger=Level (1) for INIT
    //              Level
    //                   ^^^          Delivery Mode=101 (INIT)
    send_ipi(apic_id, 0x00004500);

    // 10ms delay (INIT requires 10ms)
    for _ in 0..100000 {
        core::hint::spin_loop();
    }

    // De-assert INIT
    // 0x4500 with Level=0: 0x00004500 & !(1<<14) = 0x00004100 doesn't work
    // Actually for INIT de-assert, we send 0x4500 with Level=0
    // But simpler: just wait, the LAPIC handles this
    // Let's skip de-assert for now

    let phys_access = (0x8000 + hhdm_offset) as *const u8;
    let phys_byte = unsafe { core::ptr::read_volatile(phys_access) };

    // Virtual access (identity mapping should exist)
    let virt_access = 0x8000 as *const u8;
    // WARNING: This will page fault if not identity mapped!
    // But we can catch that...
    serial_println!("Attempting to read from virtual 0x8000 (tests identity mapping)...");
    // Try reading - if this crashes, identity mapping is missing
    let virt_byte = unsafe { core::ptr::read_volatile(virt_access) };
    serial_println!(
        "Virtual 0x8000: {:#04x} (physical: {:#04x})",
        virt_byte,
        phys_byte
    );

    // Send first SIPI
    serial_println!("Core {}: Sending first SIPI", core_id);
    let sipi_vector = (TRAMPOLINE_PHYS >> 12) as u32 & 0xFF;

    // SIPI IPI: Delivery Mode=110 (Startup), Physical, Edge, De-assert
    // 0x4600 = 0b0100_0110_0000_0000
    //              ^^   ^^          Level=0 (Edge), Trigger=0 (Edge) for SIPI
    //                   ^^^         Delivery Mode=110 (Startup)
    // | vector: bits 0-7
    send_ipi(apic_id, 0x00004600 | sipi_vector);

    // Wait for AP to start (200us timeout)
    let mut started = false;
    for _ in 0..10000 {
        if core::ptr::read_volatile(magic_ptr) == 0x41505354 {
            started = true;
            break;
        }
        core::hint::spin_loop();
    }

    if !started {
        // Send second SIPI
        serial_println!("Core {}: Sending second SIPI", core_id);
        send_ipi(apic_id, 0x00004600 | sipi_vector);

        // Wait longer
        for _ in 0..100000000 {
            if core::ptr::read_volatile(magic_ptr) == 0x41505354 {
                started = true;
                break;
            }
            core::hint::spin_loop();
        }
    }

    serial_println!("=== AP Core {} Diagnostic Data ===", core_id);

    let diag_base = (0x8FF0 + hhdm_offset) as *const u16;
    let values: [u16; 16] = core::ptr::read_volatile(diag_base as *const [u16; 16]);

    serial_println!("0x8FF0: 0x{:04X} (expected 0xDEAD)", values[0]);
    serial_println!("0x8FF2: 0x{:04X} (expected 0xBEEF)", values[1]);
    serial_println!("0x8FF4: 0x{:04X} (expected 0xCAFE)", values[2]);
    serial_println!("0x8FF6: 0x{:04X} (expected 0xC001)", values[3]);
    serial_println!("0x8FF8: 0x{:04X} (expected 0xD0D0)", values[4]);
    serial_println!("0x8FFA: 0x{:04X} (expected 0xDA1E)", values[5]);
    serial_println!("0x8FFC: 0x{:04X} (expected 0xDB0E)", values[6]);
    serial_println!("0x8FF0: 0x{:04X} (expected 0xDD0E)", values[7]);
    serial_println!("0x8FFE: 0x{:04X} (expected 0xDC0E)", values[8]);

    // Check 64-bit marker
    let diag64 = (ap_stack.top()) as *const u32;
    let val64 = core::ptr::read_volatile(diag64);
    serial_println!(" ap_stack.top(): 0x{:08X} (expected 0x6464B007)", val64);

    // Determine where we failed
    // if values[0] != 0xDEAD {
    //     serial_println!("AP NEVER STARTED - SIPI not received!");
    // } else if values[3] == 0 {
    //     serial_println!("AP crashed in 16-bit mode (GDT or CR0)");
    // } else if values[4] == 0 {
    //     serial_println!("AP crashed entering 32-bit mode");
    // } else if values[5] == 0 {
    //     serial_println!("AP crashed loading CR3");
    // } else if values[6] == 0 {
    //     serial_println!("AP crashed enabling long mode");
    // } else if values[7] == 0 {
    //     serial_println!("AP crashed enabling paging (likely missing identity mapping)");
    // } else if val64 == 0x6464B007 {
    //     serial_println!("SUCCESS! AP reached 64-bit mode!");
    // }

    if !started {
        serial_println!("Core {}: AP failed to start after SIPIs", core_id);
        return Err("AP failed to start");
    }

    // Acknowledge the AP
    //core::ptr::write_volatile(ack_ptr, 0x1);
    core::ptr::write_volatile(0x8FF4 as *mut u32, 0x1);
    serial_println!("Core {}: Sent acknowledgment to AP", core_id);

    // Wait for AP to finish with trampoline
    for _ in 0..100000000 {
        //if core::ptr::read_volatile(done_ptr as *const u16) == 0x2222 {
        if core::ptr::read_volatile(0x8FF8 as *const u16) == 0x2222 {
            serial_println!("Core {}: AP signaled trampoline completion", core_id);
            break;
        }
        core::hint::spin_loop();
    }

    let entry_point_value = core::ptr::read_volatile(0x8F30 as *const u64);
    serial_println!(
        "0x8F30 (Entry Point): 0x{:016X} (expected non-zero)",
        entry_point_value
    );
    serial_println!("actual ap_entry_point: {:#x}", ap_core_entry_point as u64);

    let rsp_value = core::ptr::read_volatile(0x8F40 as *const u64);
    serial_println!(
        "0x8F40 (Stack Pointer): 0x{:016X} (expected non-zero)",
        rsp_value
    );
    serial_println!("actual stack ptr: {:#x}", ap_stack.top());

    serial_println!("Core {}: AP successfully started", core_id);

    let stack_final_rsp_ap = core::ptr::read_volatile(0x9068 as *const u64);
    let trampoline_written_cr3 = core::ptr::read_volatile(0x9060 as *const u64);
    serial_println!(
        "Core {}: AP signaled trampoline completion, cr 3 written: {:#x}",
        core_id,
        trampoline_written_cr3
    );
    serial_println!(
        "Core {}: AP final RSP value: {:#x}",
        core_id,
        stack_final_rsp_ap
    );

    Ok(())
}

unsafe fn send_ipi(apic_id: u8, vector: u32) {
    let lapic = get_lapic_base_addr();
    let icr_low = lapic.offset(0x300 / 4);
    let icr_high = lapic.offset(0x310 / 4);

    serial_println!("  send_ipi: apic_id={}, vector={:#x}", apic_id, vector);

    // Wait for previous IPI to complete
    serial_println!("  send_ipi: waiting for idle...");
    let mut timeout = 0;
    while (icr_low.read_volatile() & (1 << 12)) != 0 {
        core::hint::spin_loop();
        timeout += 1;
        if timeout > 1000000 {
            serial_println!("  send_ipi: TIMEOUT waiting for idle!");
            break;
        }
    }
    serial_println!("  send_ipi: ICR idle (timeout={})", timeout);

    // Set destination APIC ID
    serial_println!("  send_ipi: setting destination to {}", apic_id);
    icr_high.write_volatile((apic_id as u32) << 24);
    serial_println!("  send_ipi: destination set");

    // Verify it was set
    let verify_high = icr_high.read_volatile();
    serial_println!(
        "ICR high after set: {:#x} (expected {:#x})",
        verify_high,
        (apic_id as u32) << 24
    );

    // Send IPI
    serial_println!(
        "  send_ipi: writing {:#x} to ICR low at {:p}",
        vector,
        icr_low
    );
    icr_low.write_volatile(vector);
    serial_println!("  send_ipi: write complete");
}
/// Send IPI to a specific APIC ID
// unsafe fn send_ipi(apic_id: u8, vector: u32) {
//     let lapic = get_lapic_base_addr();
//     let icr_low = lapic.offset(APICOffset::Icr1 as isize / 4);
//     let icr_high = lapic.offset(APICOffset::Icr2 as isize / 4);

//     // Wait for previous IPI to complete (Delivery Status bit 12 must be 0)
//     while (icr_low.read_volatile() & (1 << 12)) != 0 {
//         core::hint::spin_loop();
//     }

//     // Set destination APIC ID in bits 24-31 of ICR high
//     icr_high.write_volatile((apic_id as u32) << 24);

//     // Write ICR low - the order matters! Must write low last to trigger IPI
//     // Bit 0-7: Vector
//     // Bit 8-10: Delivery Mode (000=Fixed, 101=INIT, 110=Startup/SIPI)
//     // Bit 11: Destination Mode (0=Physical, 1=Logical)
//     // Bit 12: Delivery Status (Read only, 0=Idle)
//     // Bit 14: Level (0=De-assert, 1=Assert) - only for INIT
//     // Bit 15: Trigger Mode (0=Edge, 1=Level)

//     icr_low.write_volatile(vector);
// }
///  Send IPI to a specific APIC ID
// unsafe fn send_ipi(apic_id: u8, vector: u32) {
//     let lapic = get_lapic_base_addr();
//     let icr_low = lapic.offset(APICOffset::Icr1 as isize / 4);
//     let icr_high = lapic.offset(APICOffset::Icr2 as isize / 4);

//     // Wait for previous IPI to complete
//     while (icr_low.read_volatile() & (1 << 12)) != 0 {
//         core::hint::spin_loop();
//     }

//     // Set destination APIC ID
//     icr_high.write_volatile((apic_id as u32) << 24);

//     // Send IPI (Delivery mode = Fixed, Destination mode = Physical, Level = Assert)
//     icr_low.write_volatile(0x00004000 | vector); // 0x4000 = Physical destination mode
// }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_core_entry_point() -> ! {
    //     let bsp_cr3: u64;
    // asm!("mov {}, cr3", out(reg) bsp_cr3, options(nostack));

    // core::ptr::write_volatile(MAGIC_OFFSET as *mut u64, AP_CORE_FUNCTION_ACHIEVED);
    // core::ptr::write_volatile(AP_CORE_CR3_MESSAGE_OFFSET as *mut u64, bsp_cr3);

    // STEP 1: Get current APIC ID WITHOUT accessing globals
    // This is safe because we're just reading MSR and performing simple math
    let apic_id = {
        let apic_base_phys = {
            let low: u32;
            let high: u32;
            core::arch::asm!(
                "rdmsr",
                in("ecx") 0x1B_u32,  // IA32_APIC_BASE MSR
                out("eax") low,
                out("edx") high,
                options(nostack, preserves_flags)
            );
            ((high as u64) << 32) | (low as u64)
        };
        let apic_base_phys = apic_base_phys & 0xFFFFF000;

        // Get HHDM offset from boot_info - this should be safe
        //let hhdm_offset = crate::boot_info::boot_info().hhdm_offset;
        let hhdm_offset = 0xFFFF800000000000;
        let apic_virt = apic_base_phys + hhdm_offset;

        // Read APIC ID from offset 0x20
        let apic_id_reg = (apic_virt + 0x20) as *const u32;
        let apic_id_val = core::ptr::read_volatile(apic_id_reg);
        ((apic_id_val >> 24) & 0xFF) as u8
    };

    core::ptr::write_volatile(MAGIC_OFFSET as *mut u64, AP_CORE_FUNCTION_ACHIEVED);
    core::ptr::write_volatile(AP_CORE_APIC_ID_MESSAGE_OFFSET as *mut u64, apic_id as u64);

    use x86_64::registers::control::Cr3;
    // let (pml4_frame, _) = Cr3::read();

    loop {
        core::arch::asm!("hlt");
    }

    // STEP 2: Initialize per-core GDT and TSS BEFORE any globals access
    // This ensures proper exception handling with IST stacks
    crate::gdt::init_ap_core(apic_id);

    let bsp_cr3: u64;
    asm!("mov {}, cr3", out(reg) bsp_cr3, options(nostack));
    core::ptr::write_volatile(MAGIC_OFFSET as *mut u64, AP_CORE_FUNCTION_ACHIEVED);
    core::ptr::write_volatile(AP_CORE_CR3_MESSAGE_OFFSET as *mut u64, bsp_cr3);

    // STEP 3: Load IDT for this core
    // This sets up exception handlers after we have a proper TSS

    interrupts::init_idt();

    // STEP 4: Now we can safely access globals since exceptions are properly handled

    // Debug: Check page table
    let (pml4_frame, _) = Cr3::read();
    let pml4_addr = pml4_frame.start_address().as_u64();
    serial_println!("AP Core {}: Started, CR3={:#x}", apic_id, pml4_addr);

    // Halt for now - further initialization should happen here
    loop {
        core::arch::asm!("hlt");
    }
}

/// Scheduler loop for AP cores
fn ap_core_scheduler_loop(core_id: u8) -> ! {
    serial_println!("Core {}: Entering scheduler loop", core_id);

    loop {
        let next_thread = {
            let mut scheduler = SCHEDULER.lock();
            scheduler.get_next_on_core(core_id)
        };

        match next_thread {
            Some(pid) => {
                serial_println!("Core {}: Running process {}", core_id, pid);

                // Set as current and run
                {
                    let mut scheduler = SCHEDULER.lock();
                    scheduler.set_current_on_core(core_id, pid);
                }

                // Restore and run the process
                //restore_thread_context(pid);
            }
            None => {
                // No work, halt until interrupt
                unsafe {
                    asm!("hlt", options(nomem, nostack));
                }
            }
        }
    }
}

/// PRITNING
use alloc::string::String;
impl CpuFeatureFlags {
    pub fn to_feature_names(&self) -> Vec<&'static str> {
        let mut features = Vec::new();

        if self.contains(CpuFeatureFlags::APIC) {
            features.push("APIC");
        }
        if self.contains(CpuFeatureFlags::X2APIC) {
            features.push("X2APIC");
        }
        if self.contains(CpuFeatureFlags::TSC) {
            features.push("TSC");
        }
        if self.contains(CpuFeatureFlags::TSC_DEADLINE) {
            features.push("TSC Deadline");
        }
        if self.contains(CpuFeatureFlags::PGE) {
            features.push("PGE");
        }
        if self.contains(CpuFeatureFlags::PAT) {
            features.push("PAT");
        }
        if self.contains(CpuFeatureFlags::SSE) {
            features.push("SSE");
        }
        if self.contains(CpuFeatureFlags::SSE2) {
            features.push("SSE2");
        }
        if self.contains(CpuFeatureFlags::SSE3) {
            features.push("SSE3");
        }
        if self.contains(CpuFeatureFlags::SSE4_1) {
            features.push("SSE4.1");
        }
        if self.contains(CpuFeatureFlags::SSE4_2) {
            features.push("SSE4.2");
        }
        if self.contains(CpuFeatureFlags::AVX) {
            features.push("AVX");
        }
        if self.contains(CpuFeatureFlags::AES) {
            features.push("AES");
        }
        if self.contains(CpuFeatureFlags::RDRAND) {
            features.push("RDRAND");
        }
        if self.contains(CpuFeatureFlags::HYPERVISOR) {
            features.push("Hypervisor");
        }

        features
    }
}

impl core::fmt::Display for CpuInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "CPU Information:")?;
        writeln!(f, "  Vendor: {}", self.vendor)?;
        writeln!(
            f,
            "  Family: {}, Model: {}, Stepping: {}",
            self.family, self.model, self.stepping
        )?;
        writeln!(f, "  APIC ID: {}", self.apic_id)?;
        writeln!(f, "  Cache Line Size: {} bytes", self.cache_line_size)?;

        writeln!(f, "  Features:")?;
        let feature_names = self.features.to_feature_names();
        if feature_names.is_empty() {
            writeln!(f, "    <none detected>")?;
        } else {
            // Print features in columns of 4 for readability
            for chunk in feature_names.chunks(4) {
                write!(f, "    ")?;
                for (i, feature) in chunk.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{:<12}", feature)?;
                }
                writeln!(f)?;
            }
        }

        Ok(())
    }
}

impl CpuInfo {
    pub fn to_pretty_string(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        write!(&mut s, "  {}", self).unwrap();
        s
    }

    pub fn to_compact_string(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let features: Vec<&str> = self.features.to_feature_names();
        let features_str = if features.is_empty() {
            String::from("none")
        } else {
            features.join("")
        };
        write!(
            &mut s,
            "CPU: {} (Family {}, Model {}), APIC ID: {}, Cache: {}B, Features: [{}]",
            self.vendor, self.family, self.model, self.apic_id, self.cache_line_size, features_str
        )
        .unwrap();
        s
    }
}
