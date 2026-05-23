use crate::serial_println;
use lazy_static::lazy_static;
use x86_64::{
    VirtAddr,
    instructions::tables::load_tss,
    registers::segmentation::{CS, DS, ES, SS, Segment},
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        tss::TaskStateSegment,
    },
};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const MAX_AP_CORES: usize = 16;

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        const STACK_SIZE: usize = 4 * 1024 * 1024;

        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            //let stack_start = VirtAddr::new(HEAP_POINTER as u64);

            stack_start + STACK_SIZE as u64
        };

        let val = tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize];
        serial_println!(
            "Initializing TSS: interrupt_stack_table[double_fault]: {:#x}",
            val
        );

        tss
    };
}

struct Gdt {
    table: GlobalDescriptorTable,
    selectors: Selectors,
}

lazy_static! {
    static ref GDT: Gdt = {
        let mut table = GlobalDescriptorTable::new();
        let kernel_code_selector = table.append(Descriptor::kernel_code_segment());
        let kernel_data_selector = table.append(Descriptor::kernel_data_segment());

        let user_code_selector = table.append(Descriptor::user_code_segment());
        let user_data_selector = table.append(Descriptor::user_data_segment());

        let tss_selector = table.append(Descriptor::tss_segment(&TSS));

        Gdt {
            table,
            selectors: Selectors {
                kernel_code_selector,
                kernel_data_selector,
                user_code_selector,
                user_data_selector,
                tss_selector,
            },
        }
    };
}

struct Selectors {
    kernel_code_selector: SegmentSelector,
    kernel_data_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

pub fn init() {
    serial_println!("Initializing GDT");
    GDT.table.load();
    // SAFETY: We have just loaded the GDT, so the selectors are valid.
    unsafe {
        CS::set_reg(GDT.selectors.kernel_code_selector);
        SS::set_reg(GDT.selectors.kernel_data_selector);
        DS::set_reg(GDT.selectors.kernel_data_selector);
        ES::set_reg(GDT.selectors.kernel_data_selector);
        load_tss(GDT.selectors.tss_selector);
    }

    serial_println!("GDT initialized:");
    serial_println!(
        "  Kernel code selector: {:?}",
        GDT.selectors.kernel_code_selector
    );
    serial_println!(
        "  Kernel data selector: {:?}",
        GDT.selectors.kernel_data_selector
    );
    serial_println!(
        "  User code selector: {:?}",
        GDT.selectors.user_code_selector
    );
    serial_println!(
        "  User data selector: {:?}",
        GDT.selectors.user_data_selector
    );
    serial_println!("  TSS selector: {:?}", GDT.selectors.tss_selector);
}

pub fn get_user_code_selector() -> SegmentSelector {
    GDT.selectors.user_code_selector
}

pub fn get_user_data_selector() -> SegmentSelector {
    GDT.selectors.user_data_selector
}

pub fn get_kernel_code_selector() -> SegmentSelector {
    GDT.selectors.kernel_code_selector
}

pub fn get_kernel_data_selector() -> SegmentSelector {
    GDT.selectors.kernel_data_selector
}

use core::{arch::asm, ptr};

pub unsafe fn init_ap_core(core_id: u8)  {
    let mut tss: TaskStateSegment = TaskStateSegment::new();
    
    const IST_STACK_BASE: u64 = 0xFFFF_FFFF_FF00_0000;
    const IST_STACK_SIZE: u64 = 4 * 1024;
    
    let ist_top = IST_STACK_BASE + ((core_id as u64) * IST_STACK_SIZE) + IST_STACK_SIZE;
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = VirtAddr::new(ist_top);
    
    // Debug: Check what GlobalDescriptorTable actually looks like in memory
    let mut gdt = GlobalDescriptorTable::new();
    
    // Print debug info about the GDT structure
    let gdt_addr = &gdt as *const GlobalDescriptorTable as u64;
    let gdt_size = core::mem::size_of::<GlobalDescriptorTable>();
    
    // Try to read the internal state - this might be the issue
    // GlobalDescriptorTable is typically:
    // struct GlobalDescriptorTable {
    //     table: [u64; MAX_ENTRIES],
    //     len: usize,
    //     next_free: usize,  // or similar tracking fields
    // }
    
    // Let's try to dump the raw bytes
    unsafe {
        let bytes = core::slice::from_raw_parts(gdt_addr as *const u8, gdt_size);
        // This is where you'd print these bytes to serial to debug
    }
    
    // Actually, the issue might be that GlobalDescriptorTable::new() 
    // panics or crashes because it needs to allocate or access globals
    
    // Let's try to manually construct the GDT without using the struct at all
    // This completely avoids GlobalDescriptorTable
    
    // Manual GDT construction:
    // Each entry is 8 bytes (u64)
    // Standard layout: [null, kernel_code, kernel_data, user_code, user_data, tss_low, tss_high]
    #[repr(align(8))]
    struct GdtEntries([u64; 7]);
    
    let mut entries = GdtEntries([
        0,  // null descriptor
        0,  // kernel code (will set below)
        0,  // kernel data
        0,  // user code
        0,  // user data
        0,  // TSS low
        0,  // TSS high
    ]);
    
    // Create segment descriptors manually
    let make_segment_descriptor = |present: bool, dpl: u8, executable: bool, writable: bool, long_mode: bool| -> u64 {
        let mut desc = 0u64;
        if present { desc |= 1u64 << 47; }
        desc |= ((dpl as u64) & 0x3) << 45;
        desc |= 1u64 << 44; // descriptor type: 1 = code/data segment
        if executable {
            desc |= 1u64 << 43; // executable
            desc |= 1u64 << 41; // readable (for code segments)
            if long_mode { desc |= 1u64 << 53; } // 64-bit
        } else {
            if writable { desc |= 1u64 << 41; } // writable (for data segments)
        }
        desc |= 1u64 << 53; // Set limit and other flags for flat memory model
        desc
    };
    
    // Entry 1: Kernel code (ring 0)
    entries.0[1] = make_segment_descriptor(true, 0, true, false, true);
    
    // Entry 2: Kernel data (ring 0)
    entries.0[2] = make_segment_descriptor(true, 0, false, true, false);
    
    // Entry 3: User code (ring 3)
    entries.0[3] = make_segment_descriptor(true, 3, true, false, true);
    
    // Entry 4: User data (ring 3)
    entries.0[4] = make_segment_descriptor(true, 3, false, true, false);
    
    // Entry 5-6: TSS descriptor (16 bytes, 2 entries)
    let tss_addr = &tss as *const TaskStateSegment as u64;
    let tss_limit = (core::mem::size_of::<TaskStateSegment>() - 1) as u64;
    
    // Low 8 bytes of TSS descriptor
    entries.0[5] = (tss_limit & 0xFFFF)
        | ((tss_addr & 0xFFFF) << 16)
        | (((tss_addr >> 16) & 0xFF) << 32)
        | (0b1001u64 << 40)  // Type: 0b1001 = 64-bit TSS (Available)
        | (1u64 << 47)       // Present
        | ((tss_limit & 0xF0000) << 48)
        | (((tss_addr >> 24) & 0xFF) << 56);
    
    // High 8 bytes of TSS descriptor
    entries.0[6] = tss_addr >> 32;
    
    // GDTR structure: packed struct { u16 limit, u64 base }
    let gdt_limit = (core::mem::size_of_val(&entries) - 1) as u16;
    let gdt_base = &entries as *const GdtEntries as u64;
    
    #[repr(C, packed)]
    struct Gdtr {
        limit: u16,
        base: u64,
    }
    
    let gdtr = Gdtr {
        limit: gdt_limit,
        base: gdt_base,
    };
    
    // Load GDT with lgdt instruction
    asm!(
        "lgdt [{}]",
        in(reg) &gdtr,
        options(readonly, nostack)
    );
    
    // Now reload segment registers
    // CS = 0x08 (entry 1, ring 0)
    // DS/SS/ES = 0x10 (entry 2, ring 0)
asm!(
    // Load data segments directly
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax", 
    "mov ss, ax",
    // For CS, we need a far return/jump
    // Use a far return to reload CS
    "push 0x08",     // new CS
    "lea {tmp}, [2f + rip]", // address of label 2
    "push {tmp}",    // new RIP
    "retfq",         // far return to reload CS
    "2:",            // we end up here with new CS
    tmp = in(reg) 0u64,
    options(nostack)
);
    
    // Load TSS - TR = 0x28 (entry 5, ring 0)
    asm!(
        "ltr ax",
        in("ax") 0x28u16,
        options(nostack)
    );
    

}