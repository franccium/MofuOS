use core::arch::asm;
use core::cell::UnsafeCell;
use x86_64::VirtAddr;
use x86_64::registers::segmentation::Segment;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

use crate::{MAX_CORES, serial_println, serial_println_core};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

// Stack size for each per-core stack (RSP0 and double-fault IST).
// 32 KiB is enough for deeply-nested kernel frames.
const PER_CORE_STACK_SIZE: usize = 8 * 4096; // 32 KiB

// Backing storage for per-core kernel stacks.
// These live in .bss, are automatically mapped by the bootloader, and stay
// alive for the lifetime of the kernel — exactly what the TSS raw pointers need.
//
// Layout per core:
//   [0..PER_CORE_STACK_SIZE]              → RSP0 stack  (ring 0 interrupt stack)
//   [PER_CORE_STACK_SIZE..2*STACK_SIZE]   → IST[0] stack (double-fault stack)
#[repr(align(16))]
struct KernelStack([u8; PER_CORE_STACK_SIZE]);

static mut RSP0_STACKS: [KernelStack; MAX_CORES as usize] =
    [const { KernelStack([0u8; PER_CORE_STACK_SIZE]) }; MAX_CORES as usize];

static mut IST0_STACKS: [KernelStack; MAX_CORES as usize] =
    [const { KernelStack([0u8; PER_CORE_STACK_SIZE]) }; MAX_CORES as usize];

static mut PER_CORE_GDT: [Gdt; MAX_CORES as usize] = {
    const EMPTY: Gdt = Gdt::empty();
    [EMPTY; MAX_CORES as usize]
};

struct Gdt {
    table: GlobalDescriptorTable,
    kernel_code_selector: SegmentSelector,
    kernel_data_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
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
            user_data_selector,
            user_code_selector,
            tss_selector,
        }
    }

    fn new(core_id: u8) -> Self {
        let idx = core_id as usize;
        let tss = unsafe { &mut *PER_CORE_TSS[idx].get() };

        // RSP0: used by the CPU on any ring-3→ring-0 transition (interrupts,
        // exceptions, syscalls via INT).  Without this, the hardware tries to
        // switch to stack address 0x0, which is unmapped → immediate triple fault.
        let rsp0_top = unsafe {
            let stack = &RSP0_STACKS[idx].0;
            // Stack grows downward; top = one-past-end of the array.
            stack.as_ptr().add(PER_CORE_STACK_SIZE) as u64
        };
        tss.privilege_stack_table[0] = VirtAddr::new(rsp0_top);

        // IST[0]: dedicated stack for the double-fault handler.  Without this
        // the double-fault handler runs on whatever (possibly corrupt) RSP it
        // inherited, which immediately causes another fault → triple fault.
        let ist0_top = unsafe {
            let stack = &IST0_STACKS[idx].0;
            stack.as_ptr().add(PER_CORE_STACK_SIZE) as u64
        };
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = VirtAddr::new(ist0_top);

        serial_println!(
            "Core {}: TSS RSP0={:#x}  IST0={:#x}",
            core_id, rsp0_top, ist0_top
        );

        let mut table = GlobalDescriptorTable::new();
        let kernel_code_selector = table.append(Descriptor::kernel_code_segment());
        let kernel_data_selector = table.append(Descriptor::kernel_data_segment());
        let user_data_selector = table.append(Descriptor::user_data_segment());
        let user_code_selector = table.append(Descriptor::user_code_segment());
        let tss_selector = table.append(Descriptor::tss_segment(tss));

        Gdt {
            table,
            kernel_code_selector,
            kernel_data_selector,
            user_data_selector,
            user_code_selector,
            tss_selector,
        }
    }
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
    let core_id = crate::util::cpuinfo::get_current_core_id() as usize;
    unsafe { PER_CORE_GDT[core_id].kernel_code_selector }
}

pub fn get_kernel_data_selector() -> SegmentSelector {
    let core_id = crate::util::cpuinfo::get_current_core_id() as usize;
    unsafe { PER_CORE_GDT[core_id].kernel_data_selector }
}

pub fn get_user_code_selector() -> SegmentSelector {
    let core_id = crate::util::cpuinfo::get_current_core_id() as usize;
    unsafe { PER_CORE_GDT[core_id].user_code_selector }
}

pub fn get_user_data_selector() -> SegmentSelector {
    let core_id = crate::util::cpuinfo::get_current_core_id() as usize;
    unsafe { PER_CORE_GDT[core_id].user_data_selector }
}

pub fn get_tss_selector() -> SegmentSelector {
    let core_id = crate::util::cpuinfo::get_current_core_id() as usize;
    unsafe { PER_CORE_GDT[core_id].tss_selector }
}
