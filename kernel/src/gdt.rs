use core::arch::asm;
use core::cell::UnsafeCell;
use x86_64::VirtAddr;
use x86_64::registers::segmentation::Segment;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

use crate::{MAX_CORES, serial_println, serial_println_core};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const PER_CORE_STACK_SIZE: u64 = 8 * 4096;
const STACK_BASE: u64 = 0xFFFF_FFFF_FF00_0000;

static mut PER_CORE_GDT: [Gdt; MAX_CORES as usize] = {
    const EMPTY: Gdt = Gdt::empty();
    [EMPTY; MAX_CORES as usize]
};

struct Gdt {
    table: GlobalDescriptorTable,
    kernel_code_selector: SegmentSelector,
    kernel_data_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

static mut PER_CORE_TSS: [UnsafeCell<TaskStateSegment>; MAX_CORES as usize] =
    unsafe { [const { UnsafeCell::new(TaskStateSegment::new()) }; MAX_CORES as usize] };

impl Gdt {
    const fn empty() -> Self {
        let mut table = GlobalDescriptorTable::new();
        let kernel_code_selector = table.append(Descriptor::kernel_code_segment());
        let kernel_data_selector = table.append(Descriptor::kernel_code_segment());
        let user_code_selector = table.append(Descriptor::kernel_code_segment());
        let user_data_selector = table.append(Descriptor::kernel_code_segment());
        let tss_selector = table.append(Descriptor::kernel_code_segment());

        Gdt {
            table,
            kernel_code_selector,
            kernel_data_selector,
            user_code_selector,
            user_data_selector,
            tss_selector,
        }
    }

    fn new(core_id: u8) -> Self {
        let tss = unsafe { &mut *PER_CORE_TSS[core_id as usize].get() };

        let ist_stack_addr = allocate_per_core_stack(core_id);
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = VirtAddr::new(ist_stack_addr);

        let mut table = GlobalDescriptorTable::new();
        let kernel_code_selector = table.append(Descriptor::kernel_code_segment());
        let kernel_data_selector = table.append(Descriptor::kernel_data_segment());
        let user_code_selector = table.append(Descriptor::user_code_segment());
        let user_data_selector = table.append(Descriptor::user_data_segment());
        let tss_selector = table.append(Descriptor::tss_segment(tss));

        Gdt {
            table,
            kernel_code_selector,
            kernel_data_selector,
            user_code_selector,
            user_data_selector,
            tss_selector,
        }
    }
}

/// Returns the top of the stack
fn allocate_per_core_stack(core_id: u8) -> u64 {
    STACK_BASE + ((core_id as u64 + 1) * PER_CORE_STACK_SIZE)
}

pub unsafe fn init_core_gdt(core_id: u8) {

    let gdt = Gdt::new(core_id);

    let idx = core_id as usize;
    if idx < MAX_CORES as usize {
        unsafe {
            PER_CORE_GDT[idx] = gdt;
        }
    }

    let gdt_ref = unsafe { &PER_CORE_GDT[idx] };

    gdt_ref.table.load();

    // Reload segment registers
    unsafe {
        // Reload code segment with a far jump
        asm!(
            "push {sel}",
            "lea {tmp}, [2f + rip]",
            "push {tmp}",
            "retfq",
            "2:",
            sel = in(reg) gdt_ref.kernel_code_selector.0 as u64,
            tmp = in(reg) 0u64,
            options(nostack)
        );

        // Reload data segments
        x86_64::registers::segmentation::DS::set_reg(gdt_ref.kernel_data_selector);
        x86_64::registers::segmentation::ES::set_reg(gdt_ref.kernel_data_selector);
        x86_64::registers::segmentation::SS::set_reg(gdt_ref.kernel_data_selector);

        // Load TSS
        x86_64::instructions::tables::load_tss(gdt_ref.tss_selector);
    }

}

pub fn get_kernel_code_selector() -> SegmentSelector {
    unsafe { PER_CORE_GDT[0].kernel_code_selector }
}

pub fn get_kernel_data_selector() -> SegmentSelector {
    unsafe { PER_CORE_GDT[0].kernel_data_selector }
}

pub fn get_user_code_selector() -> SegmentSelector {
    unsafe { PER_CORE_GDT[0].user_code_selector }
}

pub fn get_user_data_selector() -> SegmentSelector {
    unsafe { PER_CORE_GDT[0].user_data_selector }
}

pub fn get_tss_selector() -> SegmentSelector {
    unsafe { PER_CORE_GDT[0].tss_selector }
}
