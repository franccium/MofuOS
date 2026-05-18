use kernel::{interrupts::map_local_apic_for_current_core, memory::memory::MemoryMapFrameAllocator, process::{CORE_POOL, CorePool, SCHEDULER}, util::cpuinfo::{self, ap_core_entry_point, init_cpu_infos, init_current_core, start_ap_core}};
use limine::{
    BaseRevision,
    framebuffer::{Framebuffer},
    paging::Mode,
    request::{
        EfiMemoryMapRequest, FramebufferRequest, HhdmRequest, MemoryMapRequest, PagingModeRequest,
        RequestsEndMarker, RequestsStartMarker, RsdpRequest, MpRequest
    },
};
use spin::Mutex;
use spin::Once;
use x86_64::VirtAddr;
use crate::main;
use kernel::{memory::allocator, init_globals, interrupts, memory, serial_println, util::cpuinfo::{init_cpu_info}, boot_info::{BOOT_INFO, BootInfo}};

/// Sets the base revision to the latest revision supported by the crate.
/// See specification for further info.
/// Be sure to mark all limine requests with #[used], otherwise they may be removed by the compiler.
#[used]
// The .requests section allows limine to find the requests faster and more safely.
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static RSDP_REUEST: RsdpRequest = RsdpRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static EFI_MEMORY_MAP_REQUEST: EfiMemoryMapRequest = EfiMemoryMapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMORY_MAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static PAGING_MODE_REQUEST: PagingModeRequest =
    PagingModeRequest::new().with_mode(Mode::FOUR_LEVEL);

#[used]
#[unsafe(link_section = ".requests")]
static MP_REQUEST: MpRequest = MpRequest::new();

/// Define the stand and end markers for Limine requests.
#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();



#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());

    serial_println!("MofuOS Booted!");


    // memory::memory::init_acpi_memory_map(rsdp_phys_addr);

    let _efi_memory_map_response = EFI_MEMORY_MAP_REQUEST
        .get_response()
        .expect("Failed to get UEFI memory map response");

    let memory_map_response = MEMORY_MAP_REQUEST
        .get_response()
        .expect("Failed to get memory map response");

    let paging_mode_response = PAGING_MODE_REQUEST
        .get_response()
        .expect("Failed to get paging mode response");
    let _paging_mode = paging_mode_response.mode();

    let hhdm_response = HHDM_REQUEST
        .get_response()
        .expect("Failed to get HHDM respone");
    let hhdm_offset = hhdm_response.offset();

    let rsdp_addr_respone = RSDP_REUEST
        .get_response()
        .expect("Failed to get RSDP address response");
    let rsdp_phys_addr: usize = rsdp_addr_respone.address();
    let rsdp_virt_addr = rsdp_phys_addr + hhdm_offset as usize;

    serial_println!("RSDP physical address: {:#x}", rsdp_phys_addr);
    serial_println!("HHDM offset: {:#x}", hhdm_offset);
    serial_println!("RSDP virtual address: {:#x}", rsdp_virt_addr);

    let mp_response = MP_REQUEST
        .get_response()
        .expect("Failed to get SMP response");

    let cpus= mp_response.cpus();
    let core_count = cpus.len();
    let bsp_lapic_id = mp_response.bsp_lapic_id();

    serial_println!("MP Info:");
    serial_println!("  Total cores: {}", core_count);
    serial_println!("  BSP LAPIC ID: {}", bsp_lapic_id);

    unsafe { init_cpu_info() };
    for (i, cpu) in cpus.iter().enumerate() {
    serial_println!("  CPU {}: LAPIC ID={}, Processor ID={}",
        i, cpu.lapic_id, cpu.id);
    }


    let mut core_pool = CORE_POOL.lock();
    core_pool.init_with_core_count(core_count as u8, cpus);
    drop(core_pool);

    let framebuffer_response = FRAMEBUFFER_REQUEST
    .get_response()
    .expect("Failed to get framebuffer response");
    let framebuffer = framebuffer_response
    .framebuffers()
    .next()
    .expect("No framebuffer found");

    let boot_info = BootInfo {
        hhdm_offset,
        framebuffer: Mutex::new(framebuffer),
    };

    BOOT_INFO.call_once(|| boot_info);
    serial_println!("Boot Info: hhdm_offset: {}", hhdm_offset);

    init_globals();

    let mut mapper = unsafe { memory::memory::init_offset_page_table(hhdm_offset) };
    serial_println!("Offset page table initialized");

    serial_println!("Creating frame_allocator");
    let mut frame_allocator =
    unsafe { MemoryMapFrameAllocator::init(memory_map_response.entries()) };

    memory::memory::map_acpi_regions(
        &mut mapper,
        &mut frame_allocator,
        rsdp_phys_addr,
        hhdm_offset,
    )
    .expect("Failed to map ACPI regions");


use x86_64::registers::control::Cr3;
let (active_pml4_frame, _) = Cr3::read();
serial_println!("1 Active PML4 frame: {:#x}", active_pml4_frame.start_address().as_u64());

    memory::memory::setup_ap_trampoline_mapping(&mut mapper, &mut frame_allocator);
    {

        // Get the currently active PML4
        let (active_pml4_frame, _) = Cr3::read();
        let active_pml4_phys = active_pml4_frame.start_address().as_u64();
        let active_pml4_virt = active_pml4_phys + hhdm_offset;

        serial_println!("Active PML4 physical: {:#x}", active_pml4_phys);
        serial_println!("Active PML4 virtual: {:#x}", active_pml4_virt);

        // Your mapper MUST point to THIS PML4!
        // Create a mapper that points to the active PML4:
        let mut active_mapper = unsafe { 
            x86_64::structures::paging::mapper::OffsetPageTable::new(
                &mut *(active_pml4_virt as *mut x86_64::structures::paging::page_table::PageTable),
                VirtAddr::new(hhdm_offset),
            )
        };

        // NOW initialize with THIS mapper:
        cpuinfo::init_ap_support(&mut active_mapper, &mut frame_allocator);
       // cpuinfo::init_ap_support(&mut mapper, &mut frame_allocator);
    }

    serial_println!("Initializing heap");
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Failed to initialize heap");
    serial_println!("Heap initialized");

    unsafe { init_cpu_infos(&cpus) };
    serial_println!("Mapping lapic for core 0");
    unsafe { map_local_apic_for_current_core(&mut mapper, &mut frame_allocator) };
    unsafe { init_current_core() };
    let mut scheduler = SCHEDULER.lock();
    scheduler.init_with_core_count(core_count as u8);
    drop(scheduler);


    unsafe {
        interrupts::init_acpi(
            rsdp_phys_addr,
            hhdm_offset,
            &mut mapper,
            &mut frame_allocator,
        )
    };

    let (kernel_page_table_frame, _) = x86_64::registers::control::Cr3::read();
    let kernel_page_table_phys = kernel_page_table_frame.start_address();
    let user_memory_manager = memory::usermem::UserMemoryManager::new(
        kernel_page_table_phys,
        hhdm_offset,
    );
    serial_println!("Global memory managers initialized");
    interrupts::enable_interrupts();
    
    /// init other cores and timers
    let hhdm_offset = BOOT_INFO.get().unwrap().hhdm_offset;
    for (idx, cpu) in cpus.iter().enumerate() {
        let core_id = cpu.id as u8;


        if core_id == 0 {
            continue;
        }

        serial_println!("Booting AP Core {} (LAPIC ID: {})", core_id, cpu.lapic_id);


        // Send INIT-SIPI-SIPI sequence to start the AP core
        // This is architecture-specific and depends on your APIC implementation
        let entry_phys = (ap_core_entry_point as u64) - hhdm_offset;


        unsafe { start_ap_core(core_id, cpu.lapic_id as u8, hhdm_offset, &mut mapper, &mut frame_allocator); }
    }

    memory::init_memory_globals(frame_allocator, user_memory_manager);

    main()
}
