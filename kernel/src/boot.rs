use acpi::rsdp;
use core::cell::UnsafeCell;
use kernel::memory::{MemoryMapFrameAllocator, STACK_SIZE};
use limine::{
    BaseRevision, framebuffer,
    framebuffer::{Framebuffer, VideoMode},
    memory_map,
    paging::Mode,
    request::{
        EfiMemoryMapRequest, FramebufferRequest, HhdmRequest, MemoryMapRequest, PagingModeRequest,
        RequestsEndMarker, RequestsStartMarker, RsdpRequest, StackSizeRequest,
    },
    response::{
        EfiMemoryMapResponse, HhdmResponse, MemoryMapResponse, RsdpResponse, StackSizeResponse,
    },
};
use spin::Mutex;
use spin::Once;
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
    structures::paging::{FrameAllocator, OffsetPageTable, PageTable, PhysFrame, Size4KiB},
};

use crate::main;
use kernel::{allocator, io::interrupts, memory, serial_println};

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
static STACK_SIZE_REQUEST: StackSizeRequest = StackSizeRequest::new().with_size(STACK_SIZE);

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

/// Define the stand and end markers for Limine requests.
#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

pub struct BootInfo {
    pub stack_size: u64,
    pub hhdm_offset: u64,
    pub paging_mode: Mode,
    pub framebuffer: Mutex<Framebuffer<'static>>,
}

pub static BOOT_INFO: Once<BootInfo> = Once::new();

pub fn boot_info() -> &'static BootInfo {
    unsafe { BOOT_INFO.get().unwrap_unchecked() }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());

    let stack_size_response: &StackSizeResponse = STACK_SIZE_REQUEST
        .get_response()
        .expect("Failed to get stack size response");

    // memory::init_acpi_memory_map(rsdp_phys_addr);

    let efi_memory_map_response = EFI_MEMORY_MAP_REQUEST
        .get_response()
        .expect("Failed to get UEFI memory map response");

    let memory_map_response = MEMORY_MAP_REQUEST
        .get_response()
        .expect("Failed to get memory map response");

    let paging_mode_response = PAGING_MODE_REQUEST
        .get_response()
        .expect("Failed to get paging mode response");
    let paging_mode = paging_mode_response.mode();

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

    let framebuffer_response = FRAMEBUFFER_REQUEST
        .get_response()
        .expect("Failed to get framebuffer response");
    let framebuffer = framebuffer_response
        .framebuffers()
        .next()
        .expect("No framebuffer found");

    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
        if let Some(framebuffer) = framebuffer_response.framebuffers().next() {
            for i in 0..100_u64 {
                // Calculate the pixel offset using the framebuffer information we obtained above.
                // We skip `i` scanlines (pitch is provided in bytes) and add `i * 4` to skip `i` pixels forward.
                let pixel_offset = i * framebuffer.pitch() + i * 4;

                // Write 0xFFFFFFFF to the provided pixel offset to fill it white.
                unsafe {
                    framebuffer
                        .addr()
                        .add(pixel_offset as usize)
                        .cast::<u32>()
                        .write(0xFFFFFFFF)
                };
            }
        }
    }

    let boot_info = BootInfo {
        stack_size: STACK_SIZE,
        hhdm_offset,
        paging_mode,
        framebuffer: Mutex::new(framebuffer),
    };

    BOOT_INFO.call_once(|| boot_info);

    let mut mapper = unsafe { memory::init_offset_page_table(hhdm_offset) };
    serial_println!("Offset page table initialized");

    serial_println!("Creating frame_allocator");
    let mut frame_allocator =
        unsafe { MemoryMapFrameAllocator::init(memory_map_response.entries()) };
        
    memory::map_acpi_regions(&mut mapper, &mut frame_allocator, rsdp_phys_addr, hhdm_offset).expect("Failed to map ACPI regions");
    
    serial_println!("Initializing heap");
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Failed to initialize heap");
    serial_println!("Heap initialized");


    // let rsdp_phys_page = PhysFrame::containing_address(PhysAddr::new(rsdp_phys_addr as u64));
    // let rsdp_virt_addr = VirtAddr::new(rsdp_phys_addr as u64 + hhdm_offset);

    // mapper.map_to(
    //     rsdp_phys_page,
    //     PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    // ).expect("Failed to map ACPI RSDP")
    // .flush();

    unsafe { interrupts::init_acpi(rsdp_phys_addr, hhdm_offset) };

    main()
}
