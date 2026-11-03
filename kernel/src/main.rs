#![no_std]
#![no_main]

mod boot;

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::fmt::Write;
use embedded_graphics::Drawable;
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::primitives::{
    Circle, Primitive, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle,
};
use kernel::bsp_init::{
    init_filesystem_on_bsp, init_shared_state_on_bsp, wait_for_ap_cores_blocking,
};
use kernel::data_structures::vector::Vec;
use kernel::graphics::color::{Rgba8888UNORM, rgba_to_xrgb};
use kernel::graphics::compositor::{self, Compositor};
use kernel::graphics::pipeline::{
    BlendState, PipelineState, RasterizerState, RenderMode, Vertex3D, VertexLayout,
};
use kernel::graphics::renderer::RenderContext;
use kernel::graphics::resources::{ConstantBuffer, Texture};
use kernel::graphics::shaders::{PassThroughVS, TextureSamplePS};
use kernel::graphics::window::{self, Window, WindowBuffer};
use kernel::interrupts;
use kernel::process::CORE_POOL;
use kernel::process::elf_loader::{ElfLoadError, ElfLoadInfo, TEST_ELF};
use kernel::process::shared_state::{
    get_shared_program_data_buffer, get_shared_program_data_buffer_mut,
};
use kernel::{
    filesystem::sirius::FileType, graphics::framebuffer::FrameBufferTarget,
    programs::theophe::Theophe, serial_println_core,
};
use x86_64::PhysAddr;
use x86_64::instructions::hlt;
extern crate alloc;
use kernel::tests_exp::{test_filesystem::test_filesystem, test_graphics, test_process};

#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) -> ! {
    use x86_64::instructions::{nop, port::Port};

    unsafe {
        let mut port = Port::new(0xF4);
        port.write(exit_code as u32);
    }

    loop {
        nop();
    }
}

#[panic_handler]
fn rust_panic(info: &core::panic::PanicInfo) -> ! {
    serial_println_core!("PANIC: {:#?}", info);
    exit_qemu(QemuExitCode::Failed);
}

fn main() -> ! {
    serial_println_core!("Welcome to MofuOS!");

    let _ = init_filesystem_on_bsp();
    kernel::tests_exp::test_ata::test_ata_filesystem();

    //test_process_system();

    //test_filesystem_system();

    //test_process::test_process_system();

    {
        let mut framebuffer_target = kernel::graphics::framebuffer::get_framebuffer();
        let fb = &mut *framebuffer_target;

        let fb_width = fb.width as f32;
        let fb_height = fb.height as f32;

        test_graphics::draw_shapes(fb);

        //TODO: compositor should own the framebuffer; adjust theophe to work as other processes would, with its own window backbufer
        serial_println_core!("Framebuffer size: {}x{}", fb_width, fb_height);

        init_shared_state_on_bsp();

        serial_println_core!("Shared state initialized");
        
        use kernel::graphics::color::{Rgba8888UNORM, rgba_to_xrgb};
        use kernel::graphics::compositor::{get_compositor, init_compositor};
        use kernel::graphics::window::{Window, WindowBuffer};
        //TODO: compositor should own the framebuffer; adjust theophe to work as other processes would, with its own window backbufer
        serial_println_core!("Framebuffer size: {}x{}", fb_width, fb_height);

        init_compositor(fb_width as u32, fb_height as u32);
        serial_println_core!("Compositor initialized");

        wait_for_ap_cores_blocking();
        serial_println_core!("AP cores ready, creating userspace processes");
        kernel::process_start::create_userspace_processes();

        serial_println_core!("Created initial userspace processes");

        serial_println_core!("Entering main kernel loop");

        loop {
            //serial_println_core!("loop");
            let time_start = interrupts::system_uptime_ns();
            let mut compositor = get_compositor();
            compositor.process_input_events();
            compositor.compose(fb);
            let time_end = interrupts::system_uptime_ns();
            let dt: u64 = time_end - time_start;
            // time_elapsed += dt;
            //serial_println_core!("Loop time: {} ns; {} ms", dt, dt as f32 / 1_000_000.0);
            hlt();
        }
    }

    exit_qemu(QemuExitCode::Success);
}
