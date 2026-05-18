#![allow(unused)]
use crate::io::serial;
use crate::process::syscall::init_syscall;
use crate::process::{SCHEDULER, Scheduler};
use crate::process::{
    process::INVALID_PID,
    process_manager::{ARCHE_PID, PROCESS_MANAGER},
};
use crate::util::apic::APICOffset;
use crate::util::cpuinfo::get_current_core_id;
use crate::util::msr::{msr_read, msr_write};
use crate::{
    gdt, hlt_loop,
    memory::memory::{IdendtityAcpiHandler, MemoryMapFrameAllocator},
    serial_print, serial_println,
    util::cpuinfo::{CpuFeatureFlags, get_cpu_info, get_cpu_info_for_core},
};
use acpi::{
    AcpiTables, PhysicalMapping,
    platform::interrupt::{Apic, InterruptModel, IoApic},
    platform::{AcpiMode, AcpiPlatform},
    sdt::{
        Signature,
        madt::{
            InterruptSourceOverrideEntry, IoApicEntry, LocalApicEntry, Madt, MadtEntry,
            NmiSourceEntry, PlatformInterruptSourceEntry,
        },
    },
};
use core::arch::asm;
use core::arch::x86_64::__rdtscp;
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::{
    PhysAddr, VirtAddr,
    registers::rflags::RFlags,
    structures::{
        idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
        paging::{FrameAllocator, Mapper, Page, PageTableFlags, PhysFrame, Size4KiB},
    },
};

const TIMER_DEBUG_PRINT: bool = false;
const KEYBOARD_DEBUG_PRINT: bool = false;
const TIMER_ENABLED: bool = false;

pub const TSC_MOCK_FREQUENCY: u64 = 2400000000u64;
pub const TIMER_TICK_INTERVAL_MS: u64 = 1000;
pub const TIMER_TICK_FREQ_DIVIDER: u64 = TIMER_TICK_INTERVAL_MS * 1000;
const MSR_IA32_TSC_DEADLINE: u32 = 0x6E0;

pub struct LapicPtr {
    address: *mut u32,
}
// SAFETY: The pointer is only used for memory-mapped I/O registers which are
// safe to access from multiple threads by design
unsafe impl Send for LapicPtr {}
unsafe impl Sync for LapicPtr {}

lazy_static! {
    pub static ref LAPIC_ADDRESS: Mutex<LapicPtr> = Mutex::new(LapicPtr {
        address: core::ptr::null_mut()
    });
}

pub fn init_idt() {
    serial_println!("init_idt");
    IDT.load();
}

fn disable_pic() {
    use x86_64::instructions::port::Port;
    unsafe {
        Port::<u8>::new(0xA1).write(0xFF);
    }
}

pub unsafe fn interrupt_over() {
    unsafe {
        let local_apic_ptr = LAPIC_ADDRESS.lock().address;
        local_apic_ptr
            .offset(APICOffset::Eoi as isize / 4)
            .write_volatile(0);
    }
}

unsafe fn map_apic_mem(
    phys_address: u32,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> VirtAddr {
    let physical_address = PhysAddr::new(phys_address as u64);
    let page = Page::containing_address(VirtAddr::new(physical_address.as_u64()));
    let frame = PhysFrame::containing_address(physical_address);
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

    serial_println!(
        "Mapping: phys {:#x}, virt: {:#x}",
        physical_address,
        page.start_address()
    );
    unsafe {
        mapper
            .map_to(page, frame, flags, frame_allocator)
            .expect("Mapping failed")
            .flush();
    }

    page.start_address()
}

unsafe fn init_io_apic(
    phys_address: u32,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) {
    serial_println!("Mapping IO APIC");

    let virt_addr = unsafe { map_apic_mem(phys_address, mapper, frame_allocator) };

    let io_apic_ptr = virt_addr.as_mut_ptr::<u32>();

    unsafe {
        io_apic_ptr.offset(0).write_volatile(0x12);
        io_apic_ptr
            .offset(4)
            .write_volatile(InterruptIndex::Keyboard as u8 as u32);
    }
}

unsafe fn init_local_apic(
    phys_address: u32,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) {
    serial_println!("Mapping Local APIC");

    let virt_addr = unsafe { map_apic_mem(phys_address, mapper, frame_allocator) };

    let local_apic_ptr = virt_addr.as_mut_ptr::<u32>();

    LAPIC_ADDRESS.lock().address = local_apic_ptr;

    unsafe {
        //init_timer(local_apic_ptr);
        init_keyboard(local_apic_ptr);
    }
}

/// Read the Time Stamp Counter (TSC) register
/// Returns the current cycle count since processor reset
unsafe fn tsc_read() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

unsafe fn init_timer_periodic_mode(local_apic_ptr: *mut u32) {
    let tsc_freq = TSC_MOCK_FREQUENCY;
    serial_println!("TSC Frequency: {} Hz", tsc_freq);

    unsafe {
        // determine the APIC timer's bus frequency
        // configure APIC timer in one-shot mode to measure
        let lvt_timer = local_apic_ptr.offset(APICOffset::LvtT as isize / 4);
        // use one-shot mode with divider 1 for measurement
        let tdcr = local_apic_ptr.offset(APICOffset::Tdcr as isize / 4);
        tdcr.write_volatile(0x0);
        lvt_timer.write_volatile(InterruptIndex::Timer as u32);

        // set a large initial count
        let ticr = local_apic_ptr.offset(APICOffset::Ticr as isize / 4);
        let test_count = 1_000_000;
        ticr.write_volatile(test_count);

        // wait for timer to fire, poll the timer current count reg
        let tccr = local_apic_ptr.offset(APICOffset::Tccr as isize / 4);
        let start = tsc_read();
        while tccr.read_volatile() != 0 {}
        let end = tsc_read();
        let tsc_cycles = end - start;

        let apic_bus_freq = (test_count as u64 * tsc_freq) / tsc_cycles;

        // configure periodic mode
        let tick_freq = TIMER_TICK_FREQ_DIVIDER;
        let divider = 16;
        let apic_timer_freq = apic_bus_freq / divider;
        let ticks_needed = (apic_timer_freq / tick_freq) as u32;

        let svr = local_apic_ptr.offset(APICOffset::Svr as isize / 4);
        let current_svr = svr.read_volatile();
        svr.write_volatile(current_svr | (1 << 8) | 0xFF);

        // Set divider to 16
        tdcr.write_volatile(0x3);
        // set periodic mode
        const LVTT_TSC_PERIODIC_MODE: u32 = 1 << 17;
        lvt_timer.write_volatile(InterruptIndex::Timer as u32 | LVTT_TSC_PERIODIC_MODE);
        // set tick rate
        ticr.write_volatile(ticks_needed);

        serial_println!("Timer configured in periodic mode:");
        serial_println!("  APIC Bus frequency: {} Hz", apic_bus_freq);
        serial_println!("  APIC timer frequency: {} Hz", apic_timer_freq);
        serial_println!(
            "  Ticks per interrupt: {} (set tick frequency: {} Hz)",
            ticks_needed,
            tick_freq
        );
    }
}

unsafe fn init_timer_tsc_deadline_mode_per_core(local_apic: *mut u32, core_id: u8) {
    serial_println!("Core {}: Setting up TSC-Deadline mode", core_id);

    unsafe {
        //set_tsc_aux(core_id);

        let svr = local_apic.offset(APICOffset::Svr as isize / 4);
        let current_svr = svr.read_volatile();
        svr.write_volatile(current_svr | (1 << 8) | 0xFF);
        serial_println!("Core {}: APIC enabled", core_id);

        let lvt_timer = local_apic.offset(APICOffset::LvtT as isize / 4);
        const LVTT_TSC_DEADLINE_MODE: u32 = 1 << 18;
        const LVTT_MASKED: u32 = 1 << 16;

        lvt_timer.write_volatile((InterruptIndex::Timer as u32) | LVTT_TSC_DEADLINE_MODE);
        serial_println!("Core {}: LVT Timer configured for TSC-Deadline", core_id);

        core::arch::x86_64::_mm_mfence();

        let tsc_freq = TSC_MOCK_FREQUENCY;
        serial_println!("TSC Frequency: {}", tsc_freq);
        let ticks_per_ms = tsc_freq / TIMER_TICK_FREQ_DIVIDER;
        let tsc_deadline = tsc_read() + ticks_per_ms;

        //let mut aux: u32 = 0;
        //let current_tsc = __rdtscp(&mut aux);
        //serial_println!("Current TSC: {} aux: {}", current_tsc, aux);
        let current_tsc = tsc_read();
        serial_println!("Current TSC: {:#x}", current_tsc);
        let first_deadline = current_tsc + ticks_per_ms;

        msr_write(MSR_IA32_TSC_DEADLINE, first_deadline);

        serial_println!("Core {}: Timer configured in TSC-Deadline mode:", core_id);
        serial_println!("  TSC Frequency: {} Hz", tsc_freq);
        serial_println!("  Timer Frequency: {} Hz", TIMER_TICK_FREQ_DIVIDER);
        serial_println!(
            "  First deadline: {} (current: {}, +{} ticks)",
            first_deadline,
            current_tsc,
            ticks_per_ms
        );
    }
}

pub unsafe fn set_tsc_aux(core_id: u8) {
    // Store core ID in low byte of TSC_AUX
    let aux_value = core_id as u64;

    // Write to IA32_TSC_AUX MSR (0xC0000103)
    let low = aux_value as u32;
    let high = (aux_value >> 32) as u32;

    asm!(
        "wrmsr",
        in("ecx") 0xC0000103_u32,
        in("eax") low,
        in("edx") high,
        options(nostack, preserves_flags)
    );
}

pub unsafe fn init_timer_for_core(core_id: u8) {
    if !TIMER_ENABLED {
        return;
    }

    serial_println!("Initializing timer for Core {}", core_id);

    let cpu_info = get_cpu_info_for_core(core_id);
    let lapic_addr = get_lapic_base_addr();

    if !cpu_info.features.contains(CpuFeatureFlags::TSC_DEADLINE) {
        serial_println!("TSC-Deadline mode not supported, falling back to periodic mode");
        unsafe { init_timer_periodic_mode(lapic_addr) };
        return;
    }

    unsafe { init_timer_tsc_deadline_mode_per_core(lapic_addr, core_id) };
}

pub unsafe fn init_timer(local_apic_ptr: *mut u32) {
    if !TIMER_ENABLED {
        return;
    }

    if !get_cpu_info()
        .features
        .contains(CpuFeatureFlags::TSC_DEADLINE)
    {
        serial_println!("TSC-Deadline mode not supported, falling back to periodic mode");
        unsafe { init_timer_periodic_mode(local_apic_ptr) };
        return;
    }

    serial_println!("TSC-Deadline mode supported, using for timer");

    unsafe {
        let svr = local_apic_ptr.offset(APICOffset::Svr as isize / 4);
        let current_svr = svr.read_volatile();
        svr.write_volatile(current_svr | (1 << 8) | 0xFF);

        let lvt_timer = local_apic_ptr.offset(APICOffset::LvtT as isize / 4);
        const LVTT_TSC_DEADLINE_MODE: u32 = 1 << 18;
        lvt_timer.write_volatile(InterruptIndex::Timer as u32 | LVTT_TSC_DEADLINE_MODE);

        use core::arch::x86_64::_mm_mfence;
        _mm_mfence();

        let tsc_freq = TSC_MOCK_FREQUENCY;
        serial_println!("TSC Frequency: {}", tsc_freq);
        let ticks_per_ms = tsc_freq / TIMER_TICK_FREQ_DIVIDER;
        let tsc_deadline = tsc_read() + ticks_per_ms;

        // write the deadline to the MSR to arm it
        msr_write(MSR_IA32_TSC_DEADLINE, tsc_deadline);
    }

    serial_println!("Timer configured in TSC-Deadline mode");
}

unsafe fn init_keyboard(local_apic_ptr: *mut u32) {
    unsafe {
        let keyboard_register = local_apic_ptr.offset(APICOffset::LvtLint1 as isize / 4);
        keyboard_register.write_volatile(InterruptIndex::Keyboard as u8 as u32);
    }
}

pub fn enable_interrupts() {
    serial_println!("Enabling interrupts");
    // Enable interrupts on the CPU
    x86_64::instructions::interrupts::enable();
    serial_println!("Interrupts enabled");
}

pub fn disable_interrupts() {
    x86_64::instructions::interrupts::disable();
}

// Store the virtual address of the mapped Local APIC
static LAPIC_VIRT_ADDR: spin::Mutex<Option<VirtAddr>> = spin::Mutex::new(None);

/// Map the Local APIC for the current core (must be called on each core)
pub unsafe fn map_local_apic_for_current_core(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> *mut u32 {
    // Read the physical address from MSR
    let lapic_phys = get_lapic_base_addr_phys();
    use x86_64::registers::control::Cr3;
    let (active_pml4_frame, _) = Cr3::read();
    serial_println!("2 Active PML4 frame: {:#x}", active_pml4_frame.start_address().as_u64());

    serial_println!("map_local_apic_for_current_core: {:#x}", lapic_phys);

    // Create a virtual address - use a fixed offset from the HHDM or map it directly
    // Option 1: Use HHDM offset if you have it (identity mapping with offset)
    // let hhdm_offset = crate::boot_info::boot_info().hhdm_offset;
    // let virt_addr = VirtAddr::new(lapic_phys + hhdm_offset);

    // Option 2: Map it explicitly (safer, works without HHDM)
    // We'll map it to a known virtual address range for APIC
    let apic_virt_base = 0xFFFF_8000_0000_0000 + 0xfee00000; // Example: high canonical address
    let page = Page::containing_address(VirtAddr::new(apic_virt_base));
    let phys_frame = PhysFrame::containing_address(PhysAddr::new(lapic_phys));
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

    unsafe {
        mapper
            .map_to(page, phys_frame, flags, frame_allocator)
            .unwrap()
            .flush();
    }

    // Store the virtual address for later use
    *LAPIC_VIRT_ADDR.lock() = Some(page.start_address());

    page.start_address().as_mut_ptr::<u32>()
}

/// Get the physical address of the Local APIC from MSR
pub fn get_lapic_base_addr_phys() -> u64 {
    const IA32_APIC_BASE_MSR: u32 = 0x1B;
    unsafe {
        let low: u32;
        let high: u32;
        asm!(
            "rdmsr",
            in("ecx") IA32_APIC_BASE_MSR,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags)
        );
        let apic_base = ((high as u64) << 32) | (low as u64);
        apic_base & 0xFFFFF000 // Page-aligned base address
    }
}

/// Get the virtual address of the Local APIC (after mapping)
pub fn get_lapic_base_addr() -> *mut u32 {
    if let Some(addr) = *LAPIC_VIRT_ADDR.lock() {
        addr.as_mut_ptr::<u32>()
    } else {
        panic!("Local APIC not mapped yet! Call map_local_apic_for_current_core first.");
    }
    // pub const LOCAL_APIC_PHYS_BASE: u64 = 0xFEE00000;
    // let hhdm_offset = crate::boot_info::boot_info().hhdm_offset;
    // let virt_addr = LOCAL_APIC_PHYS_BASE + hhdm_offset;
    // virt_addr as *mut u32
}

pub unsafe fn init_acpi(
    rsdp_addr: usize,
    phys_offset: u64,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) {
    serial_println!("init_acpi()");
    let handler = IdendtityAcpiHandler {
        phys_offset: phys_offset,
    };
    serial_println!("creating AcpiTables");
    let acpi_tables =
        unsafe { AcpiTables::from_rsdp(handler, rsdp_addr).expect("Failed to parse ACPI") };

    serial_println!("AcpiTables initialized");

    let acpi_platform: AcpiPlatform<IdendtityAcpiHandler> =
        AcpiPlatform::new(acpi_tables, handler).expect("Cannot create AcpiPlatform");

    let mut lapic_addr: u32 = 0;
    let mut io_apic_addr: u32 = 0;
    let mut got_apic_addr = false;

    match acpi_platform.interrupt_model {
        InterruptModel::Apic(apic) => {
            serial_println!("APIC supported");
            lapic_addr = apic.local_apic_address as u32;
            serial_println!("Found {} IO APICs", apic.io_apics.len());
            let io_apic = apic.io_apics.get(0).unwrap();
            let io_apic_id = io_apic.id;
            io_apic_addr = io_apic.address;
            got_apic_addr = true;
            let gsi_base = io_apic.global_system_interrupt_base;

            serial_println!("LAPIC at addr: {:#x}", lapic_addr);

            serial_println!(
                "IOAPIC {} at addr: {:#x}, GSI base {}",
                io_apic_id,
                io_apic_addr,
                gsi_base
            );
            /*
               local_apic_nmi_lines: Vec<NmiLine, A>,
               pub interrupt_source_overrides: Vec<InterruptSourceOverride, A>,
               pub nmi_sources: Vec<NmiSource, A>,
               pub also_has_legacy_pics: bool,
            */
        }
        _ => {
            serial_println!("APIC not supported");
        }
    }

    // let binding = acpi_platform
    //     .tables
    //     .find_table::<Madt>()
    //     .expect("Cannot find MADT table");
    // let madt_table = binding.get();

    // let local_apic_addr = madt_table.local_apic_address;
    // let flags = madt_table.flags;
    // serial_println!(
    //     "Found MADT table: local apic address: {:#x}, flags: {}",
    //     local_apic_addr,
    //     flags
    // );

    // for entry in madt_table.entries() {
    //     match entry {
    //         MadtEntry::LocalApic(local) => {
    //             let apic_id = local.apic_id;
    //             let processor_id = local.processor_id;

    //             serial_println!("Local APIC ID {} for CPU {}", apic_id, processor_id);
    //         }

    //         MadtEntry::IoApic(io_apic) => {
    //             io_apic_addr = io_apic.io_apic_address;
    //             got_io_apic_addr = true;
    //             let gsi_base = io_apic.global_system_interrupt_base;

    //             serial_println!(
    //                 "IOAPIC {} at addr: {:x}, GSI base {}",
    //                 io_apic.io_apic_id,
    //                 io_apic_addr,
    //                 gsi_base
    //             );
    //         }

    //         MadtEntry::InterruptSourceOverride(iso) => {
    //             let irq = iso.irq;
    //             let bus = iso.bus;
    //             let global_system_interrupt = iso.global_system_interrupt;
    //             serial_println!(
    //                 "IRQ {} on bus {} overridden to GSI {}",
    //                 irq,
    //                 bus,
    //                 global_system_interrupt
    //             );
    //         }

    //         MadtEntry::PlatformInterruptSource(e) => {
    //             // handle if needed
    //         }

    //         MadtEntry::NmiSource(nmi) => { /* handle NMI */ }

    //         _ => {}
    //     }
    // }

    unsafe {
        init_local_apic(lapic_addr, mapper, frame_allocator);
    }

    if got_apic_addr {
        unsafe {
            init_io_apic(io_apic_addr, mapper, frame_allocator);
        }
    } else {
        serial_println!("ERROR: Cannot find IO apic");
    }

    init_syscall();

    disable_pic();
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // CPU exceptions without error codes
        idt.divide_error.set_handler_fn(divide_by_zero_handler);
        idt.debug.set_handler_fn(debug_handler);
        idt.non_maskable_interrupt.set_handler_fn(nmi_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.overflow.set_handler_fn(overflow_handler);
        idt.bound_range_exceeded.set_handler_fn(bound_range_exceeded_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.device_not_available.set_handler_fn(device_not_available_handler);
        idt.alignment_check.set_handler_fn(alignment_check_handler);

        // CPU exceptions with error codes
        idt.invalid_tss.set_handler_fn(invalid_tss_handler);
        idt.segment_not_present.set_handler_fn(segment_not_present_handler);
        idt.stack_segment_fault.set_handler_fn(stack_segment_fault_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
        idt.page_fault.set_handler_fn(pagefault_handler);
        idt.simd_floating_point.set_handler_fn(simd_floating_point_handler);

        idt.security_exception.set_handler_fn(security_exception_handler);

        // Hardware interrupts
        idt[InterruptIndex::Timer as u8].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard as u8].set_handler_fn(keyboard_interrupt_handler);

        //unsafe {idt[0x80].set_handler_fn(syscall_int80_handler).set_stack_index(1)};

        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }

        idt
    };
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn divide_by_zero_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: DIVIDE BY ZERO\n{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn debug_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: DEBUG\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn nmi_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: NON-MASKABLE INTERRUPT\n{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn overflow_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: OVERFLOW\n{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: BOUND RANGE EXCEEDED\n{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: INVALID OPCODE\n{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn device_not_available_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: DEVICE NOT AVAILABLE\n{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!(
        "EXCEPTION: ALIGNMENT CHECK\nError Code: {}\n{:#?}",
        error_code,
        stack_frame
    );
    hlt_loop();
}

extern "x86-interrupt" fn invalid_tss_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    serial_println!(
        "EXCEPTION: INVALID TSS\nError Code: {}\n{:#?}",
        error_code,
        stack_frame
    );
    hlt_loop();
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!(
        "EXCEPTION: SEGMENT NOT PRESENT\nError Code: {}\n{:#?}",
        error_code,
        stack_frame
    );
    hlt_loop();
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!(
        "EXCEPTION: STACK SEGMENT FAULT\nError Code: {}\n{:#?}",
        error_code,
        stack_frame
    );
    hlt_loop();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!(
        "EXCEPTION: GENERAL PROTECTION FAULT\nError Code: {}\n{:#?}",
        error_code,
        stack_frame
    );
    hlt_loop();
}

extern "x86-interrupt" fn simd_floating_point_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: SIMD FLOATING POINT\n{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn security_exception_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    serial_println!("EXCEPTION: SECURITY EXCEPTION\nError Code: {}\n{:#?}", error_code, stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let core_id = get_current_core_id();

    if TIMER_DEBUG_PRINT {
        serial_println!("{}*", core_id);
    }

    unsafe {
        // let mut aux : u32 = 0;
        // let current_tsc = __rdtscp(&mut aux);
        // serial_println!("Core {}: Timer interrupt, TSC: {}, aux: {}", core_id, current_tsc, aux);

        // re-arm the timer for the next tick
        let next_deadline = tsc_read() + (TSC_MOCK_FREQUENCY / TIMER_TICK_FREQ_DIVIDER);
        msr_write(MSR_IA32_TSC_DEADLINE, next_deadline);

        let mut scheduler = SCHEDULER.lock();
        scheduler.on_timer_tick(core_id);

        interrupt_over();
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use pc_keyboard::{DecodedKey, HandleControl, Keyboard, ScancodeSet1, layouts};
    use spin::Mutex;
    use x86_64::instructions::port::Port;

    lazy_static! {
        static ref KEYBOARD: Mutex<Keyboard<layouts::Uk105Key, ScancodeSet1>> =
            Mutex::new(Keyboard::new(
                ScancodeSet1::new(),
                layouts::Uk105Key,
                HandleControl::Ignore
            ));
    }
    let mut keyboard = KEYBOARD.lock();
    let mut keyboard_port = Port::new(0x60);
    // SAFETY: This port is only read from in this interrupt handler.
    let scancode: u8 = unsafe { keyboard_port.read() };

    if let Ok(Some(event)) = keyboard.add_byte(scancode)
        && let Some(decoded_key) = keyboard.process_keyevent(event)
        && KEYBOARD_DEBUG_PRINT
    {
        match decoded_key {
            DecodedKey::Unicode(character) => {
                serial_print!("{}", character)
            }
            DecodedKey::RawKey(key) => {
                serial_print!("{:?}", key)
            }
        }
    }

    unsafe {
        interrupt_over();
    }
}

extern "x86-interrupt" fn pagefault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    serial_println!("EXCEPTION: PAGE FAULT");
    serial_println!("Accessed Address: {:?}", Cr2::read());
    serial_println!("Error Code: {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    hlt_loop();
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = 32,
    Keyboard,
}
