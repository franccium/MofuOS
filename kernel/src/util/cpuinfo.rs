use crate::asm::ap_trampoline;
use crate::interrupts::{
    get_lapic_base_addr, init_timer_for_core, map_local_apic_for_current_core,
};
use crate::process::{CORE_POOL, SCHEDULER};
use crate::serial_println;
use crate::util::apic::APICOffset;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::arch::asm;
use limine::mp::Cpu;
use spin::Once;

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
pub unsafe fn start_ap_core(core_id: u8, apic_id: u8, hhdm_offset: u64) {
    use x86_64::registers::control::Cr3;

    // Copy the pre-assembled trampoline binary to physical address 0x8000
    serial_println!("Core {}: Copying AP trampoline to memory", core_id);
    ap_trampoline::copy_to_memory(hhdm_offset);

    // Get the current CR3 (page table address) from the BSP
    serial_println!("Core {}: Reading CR3 from BSP", core_id);
    let (l4_table_frame, _flags) = Cr3::read();
    let cr3_phys = l4_table_frame.start_address().as_u64();

    // Write CR3 to 0x9000 so the AP can read it
    serial_println!("Core {}: Writing CR3 ({:#x}) to memory for AP", core_id, cr3_phys);
    let cr3_location = (0x9000 + hhdm_offset) as *mut u64;
    core::ptr::write_unaligned(cr3_location, cr3_phys);

    // Write the entry point address to 0x9010 so the AP can jump to it
    serial_println!(
        "Core {}: Writing AP entry point ({:#x}) to memory for AP",
        core_id,
        ap_core_entry_point as u64
    );
    let entry_point = ap_core_entry_point as *const () as u64;
    let entry_location = (0x9010 + hhdm_offset) as *mut u64;
    serial_println!("Core {}: Writing entry point {:#x} to {:#x}", core_id, entry_point, entry_location as u64);
    core::ptr::write_unaligned(entry_location, entry_point);

    // The entry point physical address is 0x8000
    let entry_phys = 0x8000;

    // Send INIT IPI
    serial_println!("Core {}: Sending INIT IPI to APIC ID {}", core_id, apic_id);
    send_ipi(apic_id, 0x0500); // INIT IPI

    // Delay
    for _ in 0..100000 {
        core::hint::spin_loop();
    }


    // Send SIPI with vector (entry_phys >> 12 = 0x8)
    serial_println!(
        "Core {}: Sending SIPI to APIC ID {} with entry point {:#x}",
        core_id,
        apic_id,
        entry_phys
    );
    send_ipi(apic_id, 0x0600 | ((entry_phys >> 12) as u32));

    // Short delay
    for _ in 0..1000 {
        core::hint::spin_loop();
    }

    // Optional second SIPI
    serial_println!(
        "Core {}: Sending second SIPI to APIC ID {} with entry point {:#x}",
        core_id,
        apic_id,
        entry_phys
    );
    send_ipi(apic_id, 0x0600 | ((entry_phys >> 12) as u32));

    serial_println!(
        "Core {}: STARTUP IPI sent (trampoline size: {} bytes, CR3={:#x})",
        core_id,
        ap_trampoline::size(),
        cr3_phys
    );
}

/// Send IPI to a specific APIC ID
unsafe fn send_ipi(apic_id: u8, vector: u32) {
    let lapic = get_lapic_base_addr();
    let icr_low = lapic.offset(APICOffset::Icr1 as isize / 4);
    let icr_high = lapic.offset(APICOffset::Icr2 as isize / 4);

    // Wait for previous IPI to complete
    while (icr_low.read_volatile() & (1 << 12)) != 0 {
        core::hint::spin_loop();
    }

    // Set destination APIC ID
    icr_high.write_volatile((apic_id as u32) << 24);

    // Send IPI (Delivery mode = Fixed, Destination mode = Physical, Level = Assert)
    icr_low.write_volatile(0x00004000 | vector); // 0x4000 = Physical destination mode
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_core_entry_point() -> ! {
    // This runs on the AP core
    let core_id = get_current_core_id();
    serial_println!("AP Core {}: Started successfully", core_id);

    // Initialize this core (CPU info, timer, etc.)
    unsafe {
        init_current_core();
    }

    // Initialize scheduler for this core
    let core_id = get_current_core_id();

    // {
    //     let mut scheduler = SCHEDULER.lock();
    //     if (core_id as usize) >= scheduler.per_core.len() {
    //         let core_count = CORE_POOL.lock().total_cores();
    //         scheduler.init_with_core_count(core_count);
    //     }
    // }

    // Mark core as available in the pool
    {
        let mut core_pool = CORE_POOL.lock();
        core_pool.mark_available(core_id);
    }

    init_timer_for_core(core_id);

    //enable_interrupts();

    // Enter the scheduler idle loop
    ap_core_scheduler_loop(core_id);
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
