use crate::{serial_println};
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
        let code_selector = table.append(Descriptor::kernel_code_segment());
        let data_selector = table.append(Descriptor::kernel_data_segment());
        let tss_selector = table.append(Descriptor::tss_segment(&TSS));

        Gdt {
            table,
            selectors: Selectors {
                code_selector,
                data_selector,
                tss_selector,
            },
        }
    };
}

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

pub fn init() {
    serial_println!("Initializing GDT");
    GDT.table.load();
    // SAFETY: We have just loaded the GDT, so the selectors are valid.
    unsafe {
        CS::set_reg(GDT.selectors.code_selector);
        SS::set_reg(GDT.selectors.data_selector);
        DS::set_reg(GDT.selectors.data_selector);
        ES::set_reg(GDT.selectors.data_selector);
        load_tss(GDT.selectors.tss_selector);
    }
}
