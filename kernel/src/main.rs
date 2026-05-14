#![no_std]
#![no_main]

mod boot;

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::fmt::Write;
use kernel::data_structures::vector::Vec;
use kernel::graphics::color::{Rgba8888UNORM, rgba_to_xrgb};
use kernel::graphics::compositor::Compositor;
use kernel::graphics::pipeline::{
    BlendState, PipelineState, RasterizerState, RenderMode, VertexLayout,
};
use kernel::graphics::renderer::RenderContext;
use kernel::graphics::resources::Texture;
use kernel::graphics::shaders::{PassThroughVS, TextureSamplePS};
use kernel::graphics::window::{Window, WindowBuffer};
use kernel::process::elf_loader::{ElfLoadError, ElfLoadInfo, TEST_ELF};
use kernel::{
    filesystem::sirius::FileType, graphics::framebuffer::FrameBufferTarget,
    programs::theophe::Theophe, serial_println,
};
use x86_64::instructions::hlt;
extern crate alloc;
use kernel::tests_exp::{test_filesystem::test_filesystem, test_graphics, test_process};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    serial_println!("PANIC: {:#?}", info);
    exit_qemu(QemuExitCode::Failed);
}

fn main() -> ! {
    serial_println!("Welcome to MofuOS!");

    kernel::process::syscall::init_syscall_stack();

    //test_filesystem_system();

    //test_process::test_process_system();
    //test_process::create_init_process();
    //test_process::create_and_run_init_process();

    let mut framebuffer_target = FrameBufferTarget::new(boot::boot_info().framebuffer.lock());

    test_graphics::draw_shapes(&mut framebuffer_target);

    let fb_width = framebuffer_target.width as f32;
    let fb_height = framebuffer_target.height as f32;

    //TODO: compositor should own the framebuffer; adjust theophe to work as other processes would, with its own window backbufer
    serial_println!("Framebuffer size: {}x{}", fb_width, fb_height);
    let mut compositor = Compositor::new(fb_width as u32, fb_height as u32);
    let (window_id, window_buffer) = compositor.create_window(600, 400, 50, 50);

    let (window3_id, window3_buffer) = compositor.create_window(400, 300, 700, 200);
    compositor.set_z_index(window3_id, 5);
    serial_println!("Created window with ID: {}", window3_id);

    //test_graphics::render_shaders(&window3_buffer);
    test_graphics::render_shaders_3d(&window3_buffer);

    // {
    //     let mut back_buffer = window3_buffer.back_buffer_mut();
    //     for y in 0..window3_buffer.height {
    //         for x in 0..window3_buffer.width {
    //             let r = (x as f32 / window3_buffer.width as f32 * 255.0) as u8;
    //             let g = (y as f32 / window3_buffer.height as f32 * 255.0) as u8;
    //             let b = 0;
    //             back_buffer.write_pixel(x, y, Rgba8888UNORM::from_rgb(r, g, b));
    //         }
    //     }

    //     serial_println!("Presenting window3 ");
    //     window3_buffer.present();
    // }

    let mut theophe = Theophe::new(window_buffer.back_buffer_mut());
    theophe.write_line("");
    theophe.write_line("  hi");
    theophe.write_line("==========================================================");
    let cpu_info = kernel::util::cpuinfo::get_cpu_info();
    let cpu_info_str = cpu_info.to_pretty_string();
    theophe.write_str(&cpu_info_str);

    theophe.render();

    compositor.focus_window(0);
    compositor.compose(&mut framebuffer_target);

    loop {
        hlt();
    }

    exit_qemu(QemuExitCode::Success);
}
