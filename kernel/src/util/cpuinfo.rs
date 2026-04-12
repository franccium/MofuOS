use crate::serial_println;
use bitflags::bitflags;
use core::arch::asm;

static mut CPU_INFO: CpuInfo = CpuInfo {
    features: CpuFeatureFlags::empty(),
    cache_line_size: 0,
    apic_id: 0,
    family: 0,
    model: 0,
    stepping: 0,
    vendor: CpuVendor::Unknown,
};

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

    (eax, ebx, ecx, edx)
}

pub unsafe fn init_cpu_info() {
    let (max_leaf, vendor_ebx, vendor_ecx, vendor_edx) = cpuid(0);
    serial_println!(
        "init_cpu_info: read leaf 0: eax: {:#x}, ebx: {:#x}, ecx: {:#x}, edx: {:#x}",
        max_leaf,
        vendor_ebx,
        vendor_ecx,
        vendor_edx
    );

    assert!(max_leaf >= 1);
    let (_, feat_ebx, feat_ecx, feat_edx) = cpuid(1);
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
    let cpu_vendor = get_vendor(vendor_ebx, vendor_ecx, vendor_edx);
    serial_println!("CPU Info:");
    serial_println!("  Vendor: {}", cpu_vendor);
    serial_println!("  Cache Line Size: {}", cache_line_size);
    serial_println!("  Features {:#b}", features);

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

pub fn get_cpu_info() -> &'static CpuInfo {
    unsafe {
        let ptr = &raw const CPU_INFO;
        &*ptr
    }
}
