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
use kernel::data_structures::vector::Vec;
use kernel::filesystem::init_filesystem_ata;
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
    get_shared_program_data_buffer, get_shared_program_data_buffer_mut, init_shared_state,
};
use kernel::util::cpuinfo::{
    AP_CORE_APIC_ID_MESSAGE_OFFSET, AP_CORE_CR3_MESSAGE_OFFSET, SECRET_MESSAGE_OFFSET,
};
use kernel::{
    filesystem::sirius::FileType, graphics::framebuffer::FrameBufferTarget,
    programs::theophe::Theophe, serial_println_core,
};
use x86_64::PhysAddr;
use x86_64::instructions::hlt;
extern crate alloc;
use kernel::tests_exp::{test_filesystem::test_filesystem, test_graphics, test_process};

const RENDER_SHADERS: bool = false;

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
    serial_println_core!("PANIC: {:#?}", info);
    exit_qemu(QemuExitCode::Failed);
}

fn main() -> ! {
    serial_println_core!("Welcome to MofuOS!");

    match init_filesystem_ata() {
        Ok(()) => serial_println_core!("OK: filesystem initialized from ATA drive"),
        Err(e) => {
            serial_println_core!("FAIL: filesystem init failed: {}", e);
        }
    }
    kernel::tests_exp::test_ata::test_ata_filesystem();

    //kernel::process_start::create_init_process();
    //kernel::process_start::create_and_run_init_process();

    //test_process_system();

    //test_filesystem_system();

    //test_process::test_process_system();
    //test_process::create_init_process();
    //test_process::create_and_run_init_process();

    {
        let mut framebuffer_target = kernel::graphics::framebuffer::get_framebuffer();
        let fb = &mut *framebuffer_target;

        let fb_width = fb.width as f32;
        let fb_height = fb.height as f32;

        test_graphics::draw_shapes(fb);

        //TODO: compositor should own the framebuffer; adjust theophe to work as other processes would, with its own window backbufer
        serial_println_core!("Framebuffer size: {}x{}", fb_width, fb_height);

        init_shared_state();

        serial_println_core!("Shared state initialized");
        // let mut compositor = Compositor::new(fb_width as u32, fb_height as u32);
        // serial_println_core!("Compositor initialized");
        // unsafe {
        //     let mut shared_program_data = get_shared_program_data_buffer_mut();
        //     shared_program_data.focused_window_id.store(window_id, Ordering::Release);
        // }

        // let (window_id, window_buffer, _event_buffer) = compositor.create_window(600, 400, 50, 50);
        // serial_println_core!("Created window with ID: {}", window_id);
        // let (window3_id, window3_buffer, _event_buffer) = compositor.create_window(400, 300, 700, 200);
        // compositor.set_z_index(window3_id, 5);
        // serial_println_core!("Created window with ID: {}", window3_id);

        // //test_graphics::render_shaders(&window3_buffer);
        // test_graphics::render_shaders_3d(&window3_buffer);

        // // {
        // //     let mut back_buffer = window3_buffer.back_buffer_mut();
        // //     for y in 0..window3_buffer.height {
        // //         for x in 0..window3_buffer.width {
        // //             let r = (x as f32 / window3_buffer.width as f32 * 255.0) as u8;
        // //             let g = (y as f32 / window3_buffer.height as f32 * 255.0) as u8;
        // //             let b = 0;
        // //             back_buffer.write_pixel(x, y, Rgba8888UNORM::from_rgb(r, g, b));
        // //         }
        // //     }

        // //     serial_println_core!("Presenting window3 ");
        // //     window3_buffer.present();
        // // }

        // let mut theophe = Theophe::new(window_buffer.back_buffer_mut());
        // theophe.write_line("");
        // theophe.write_line("  hi");
        // theophe.write_line("==========================================================");
        // let cpu_info = kernel::util::cpuinfo::get_cpu_info();
        // let cpu_info_str = cpu_info.to_pretty_string();
        // theophe.write_str(&cpu_info_str);

        // theophe.render();

        // compositor.focus_window(0);
        // //compositor.compose(&mut framebuffer_target);

        // let mut ctx = RenderContext::new();

        // // Create a checkerboard texture
        // const SIZE: u32 = 64;
        // let texture_data = alloc::vec::Vec::from(
        //     (0..(SIZE * SIZE))
        //         .map(|i| {
        //             let x = i % SIZE;
        //             let y = i / SIZE;
        //             let checker = ((x / 8) + (y / 8)) % 2 == 0;
        //             if checker {
        //                 Rgba8888UNORM::from_rgb(255, 128, 0).to_u32_rgba() // Orange
        //             } else {
        //                 Rgba8888UNORM::from_rgb(0, 128, 255).to_u32_rgba() // Blue
        //             }
        //         })
        //         .collect::<alloc::vec::Vec<u32>>(),
        // );
        // let texture = Texture::from_data(SIZE, SIZE, texture_data);
        // let texture_slot = ctx.bind_texture(texture);

        // // Set up constant buffer with MVP matrix (update this each frame for animation)
        // let mut constant_data = alloc::vec![0u8; 64]; // 4x4 matrix = 64 bytes
        // let cbuffer = ConstantBuffer::from_data(constant_data);
        // let cbuffer_slot = ctx.bind_cbuffer(cbuffer);
        // let mut obj_x = 2f32;
        // let mut obj_y = 1f32;
        // let mut obj_z = 1f32;
        // let mut angle = 0f32;
        // // compositor.compose(&mut framebuffer_target);
        // let mut back_buffer = window3_buffer.back_buffer_mut();
        // let mut render_target = ctx.begin_frame(&mut back_buffer);

        // let s = 1f32; // half-size
        // let vertices = [
        //     // Front face
        //     Vertex3D::new(-s, -s, s, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        //     Vertex3D::new(s, -s, s, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0),
        //     Vertex3D::new(s, s, s, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0),
        //     Vertex3D::new(-s, s, s, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0),
        //     // Back face
        //     Vertex3D::new(-s, -s, -s, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0),
        //     Vertex3D::new(s, -s, -s, 1.0, 1.0, 0.0, 0.0, 0.0, -1.0),
        //     Vertex3D::new(s, s, -s, 1.0, 1.0, 1.0, 0.0, 0.0, -1.0),
        //     Vertex3D::new(-s, s, -s, 1.0, 0.0, 1.0, 0.0, 0.0, -1.0),
        //     // Top face
        //     Vertex3D::new(-s, s, -s, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0),
        //     Vertex3D::new(s, s, -s, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0),
        //     Vertex3D::new(s, s, s, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0),
        //     Vertex3D::new(-s, s, s, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0),
        //     // Bottom face
        //     Vertex3D::new(-s, -s, -s, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0),
        //     Vertex3D::new(s, -s, -s, 1.0, 1.0, 0.0, 0.0, -1.0, 0.0),
        //     Vertex3D::new(s, -s, s, 1.0, 1.0, 1.0, 0.0, -1.0, 0.0),
        //     Vertex3D::new(-s, -s, s, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0),
        //     // Right face
        //     Vertex3D::new(s, -s, -s, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
        //     Vertex3D::new(s, s, -s, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0),
        //     Vertex3D::new(s, s, s, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0),
        //     Vertex3D::new(s, -s, s, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0),
        //     // Left face
        //     Vertex3D::new(-s, -s, -s, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0),
        //     Vertex3D::new(-s, s, -s, 1.0, 1.0, 0.0, -1.0, 0.0, 0.0),
        //     Vertex3D::new(-s, s, s, 1.0, 1.0, 1.0, -1.0, 0.0, 0.0),
        //     Vertex3D::new(-s, -s, s, 1.0, 0.0, 1.0, -1.0, 0.0, 0.0),
        // ];

        // let indices = [
        //     // Front (+Z)
        //     0, 1, 2, 0, 2, 3, // Back (-Z)
        //     4, 6, 5, 4, 7, 6, // Top (+Y)
        //     8, 10, 9, 8, 11, 10, // Bottom (-Y)
        //     12, 13, 14, 12, 14, 15, // Right (+X)
        //     16, 17, 18, 16, 18, 19, // Left (-X)
        //     20, 22, 21, 20, 23, 22,
        // ];

        // let mut time_elapsed = 0;
        // Rectangle::new(Point::new(0, 0), Size::new(100, 100))
        //     .into_styled(PrimitiveStyle::with_fill(Rgb888::RED))
        //     .draw(fb)
        //     .unwrap();

        // Rectangle::new(Point::new(60, 60), Size::new(100, 100))
        //     .into_styled(PrimitiveStyle::with_fill(Rgb888::GREEN))
        //     .draw(fb)
        //     .unwrap();

        // let style = PrimitiveStyleBuilder::new()
        //     .stroke_color(Rgb888::RED)
        //     .stroke_width(3)
        //     .fill_color(Rgb888::WHITE)
        //     .build();
        // for i in 0..5 {
        //     let x = (fb_width / 9.0) * (i as f32 + 1.0) - 10.0;
        //     let y = (fb_height / 9.0) * (i as f32 + 1.0);
        //     let radius = 10.0 + i as f32 * 2.5;

        //     Circle::new(Point::new(x as i32, y as i32), radius as u32)
        //         .into_styled(style)
        //         .draw(fb)
        //         .unwrap();
        // }

        // let mut vec = Vec::<i32>::with_capacity(4);
        // vec.push(1);
        // vec.push(2);
        // vec.push(3);
        // vec.push(1);
        // vec.push(2);
        // vec.push(3);
        // for i in 0..vec.size {
        //     serial_println_core!("vec[{}] = {}", i, vec.get(i));
        // }
        // serial_println_core!("Vector capacity: {}", vec.capacity);

        use kernel::graphics::color::{Rgba8888UNORM, rgba_to_xrgb};
        use kernel::graphics::compositor::{get_compositor, init_compositor};
        use kernel::graphics::window::{Window, WindowBuffer};
        //TODO: compositor should own the framebuffer; adjust theophe to work as other processes would, with its own window backbufer
        serial_println_core!("Framebuffer size: {}x{}", fb_width, fb_height);

        init_compositor(fb_width as u32, fb_height as u32);
        serial_println_core!("Compositor initialized");
        // {
        //     let mut compositor = get_compositor();
        //     let (window_id, window_buffer, _event_buffer) = compositor.create_window(20, 20, 30, 30, 0);
        //     serial_println_core!("Created window with ID: {}", window_id);
        //     compositor.focus_window(0);
        //     compositor.compose(fb);
        // }

        let core_count = CORE_POOL.lock().total_cores();
        serial_println_core!("Waiting for {} AP cores to be ready...", core_count - 1);
        while kernel::AP_CORES_READY.load(core::sync::atomic::Ordering::Acquire) < (core_count - 1)
        {
            hlt();
        }
        kernel::process_start::create_userspace_processes();

        serial_println_core!("Created initial userspace processes");

        serial_println_core!("Entering main kernel loop");

        loop {
            if RENDER_SHADERS {
                // let time_start = interrupts::system_uptime_ns();
                // ctx.clear(&mut render_target, Rgba8888UNORM::GRAY);

                // test_graphics::render_shaders_2d(&window3_buffer, &mut render_target);

                // // test_graphics::render_shaders_3d_loop(
                // //     &window3_buffer,
                // //     &mut render_target,
                // //     &mut ctx,
                // //     obj_x,
                // //     obj_y,
                // //     obj_z,
                // //     angle,
                // //     &vertices,
                // //     &indices,
                // // );
                // // //obj_y += 0.1f32;
                // // angle += 45f32;

                // window3_buffer.present();
                // let mut compositor = get_compositor();
                // compositor.process_input_events();
                // compositor.compose(fb);
                // let time_end = interrupts::system_uptime_ns();
                // let dt: u64 = time_end - time_start;
                // time_elapsed += dt;
                // serial_println_core!("Loop time: {} ns; {} ms", dt, dt as f32 / 1_000_000.0);
            } else {
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
    }

    exit_qemu(QemuExitCode::Success);
}
