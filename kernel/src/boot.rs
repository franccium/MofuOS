use kernel::memory::STACK_SIZE;
use limine::framebuffer::{Framebuffer, VideoMode};
use limine::request::{
    FramebufferRequest, RequestsEndMarker, RequestsStartMarker, StackSizeRequest,
};
use limine::response::StackSizeResponse;
use limine::{BaseRevision, framebuffer};
use spin::Once;

use crate::main;
use kernel::serial_println;

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

/// Define the stand and end markers for Limine requests.
#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

#[expect(unused)]
pub struct BootInfo {
    pub stack_size: u64,
    pub framebuffer: Framebuffer<'static>,
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
    serial_println!("Stack size set to {} bytes", stack_size_response.revision());

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
        stack_size: stack_size_response.revision(),
        framebuffer,
    };

    BOOT_INFO.call_once(|| boot_info);

    main()
}
