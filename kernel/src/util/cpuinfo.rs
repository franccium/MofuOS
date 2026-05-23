use crate::asm::ap_trampoline;
use crate::interrupts::{
    self, get_lapic_base_addr, get_lapic_base_addr_phys, init_timer_for_core,
    map_local_apic_for_current_core,
};
use crate::memory::{FRAME_ALLOCATOR, get_frame_allocator};
use crate::process::{self, CORE_POOL, SCHEDULER};
use crate::util::apic::APICOffset;
use crate::util::msr::msr_read;
use crate::{HHDM_OFFSET, gdt, hlt_loop, serial_println, serial_println_core};
use alloc::vec::Vec;
use bitflags::bitflags;
use core::arch::asm;
use limine::mp::MpInfo;
use spin::Once;
use x86_64::instructions::hlt;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, PageTable, Size4KiB, frame,
};

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

pub fn init_cpu_infos(cpus: &[&MpInfo]) {
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

    process::syscall::init_syscall();

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

pub unsafe extern "C" fn ap_core_from_limine_entry_point(cpu: &MpInfo) -> ! {
    //hlt_loop();

    let proc_id = cpu.processor_id as u8;
    let lapic_id = cpu.lapic_id as u8;
    if proc_id == 0 {
        serial_println!("BSP core entered AP entry point, this should never happen!");
        loop {
            core::arch::asm!("hlt");
        }
    }

    let lapic_base_addr = get_lapic_base_addr_phys();
    serial_println!(
        "AP core entry point reached for APIC ID {} (CPU {})",
        lapic_id,
        proc_id
    );
    serial_println!(
        "AP core {}: LAPIC base physical address: {:#x}",
        proc_id,
        lapic_base_addr
    );
    use x86_64::registers::control::Cr3;
    let (active_pml4_frame, _) = Cr3::read();
    serial_println!(
        "AP core {}: Active PML4 frame: {:#x}",
        proc_id,
        active_pml4_frame.start_address().as_u64()
    );

    serial_println!(
        "AP core {} (LAPIC ID {}) starting initialization...",
        proc_id,
        lapic_id
    );

    // Create the core's GDT
    gdt::init_core_gdt(proc_id);
    serial_println!("Core {}: GDT loaded", proc_id);

    // Load IDT
    interrupts::load_idt();
    serial_println!("Core {}: IDT loaded", proc_id);

    // we use the same page tables so we dont have to map
    // but we need to initialize this core's LAPIC registers
    {
        let phys_mem_offset = VirtAddr::new(HHDM_OFFSET);
        let pml4_virt =
            VirtAddr::new(active_pml4_frame.start_address().as_u64() as u64 + HHDM_OFFSET);
        let pml4_table = unsafe { &mut *(pml4_virt.as_u64() as *mut PageTable) };

        let mut mapper = unsafe { OffsetPageTable::new(pml4_table, phys_mem_offset) };

        let mut frame_allocator = get_frame_allocator();
        let fa = &mut *frame_allocator;

        // unsafe { map_local_apic_for_current_core(&mut mapper, fa) };
    }
    unsafe { interrupts::init_lapic_for_current_core(proc_id) };
    serial_println!("Core {}: LAPIC initialized", proc_id);

    // TODO: Initialize this core in the core pool

    // Initialize timer
    unsafe { init_current_core() };
    serial_println!("Core {}: Timer initialized", proc_id);

    serial_println!("Core {}: Enabling interrupts...", proc_id);
    interrupts::enable_interrupts();
    serial_println!("Core {}: Interrupts enabled", proc_id);

    let mut time_elapsed = 0;

    process::scheduler::run_on_core_loop(proc_id);
    // loop {
    //     core::arch::asm!("hlt");
    // }

    // loop {
    //     // let time_start = interrupts::system_uptime_ns();

    //     // let time_end = interrupts::system_uptime_ns();
    //     // let dt: u64 = time_end - time_start;
    //     // time_elapsed += dt;
    //     // serial_println_core!(
    //     //     "Loop time: {} ns; {} ms",
    //     //     dt,
    //     //     dt as f32 / 1_000_000.0
    //     // );

    //     core::arch::asm!("hlt");
    // }
}
