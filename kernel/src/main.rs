#![no_std]
#![no_main]

mod boot;

use core::arch::asm;
use core::fmt::Write;
use embedded_graphics::prelude::*;
use kernel::{
    allocator, graphics::framebuffer::FrameBufferTarget, programs::theophe::Theophe, serial_print,
    serial_println,
};
use x86_64::{instructions::hlt, structures::paging::frame};

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

    use embedded_graphics::pixelcolor::Rgb888;
    use embedded_graphics::primitives::{Circle, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle};

    let mut framebuffer_target = FrameBufferTarget::new(boot::boot_info().framebuffer.lock());

    Rectangle::new(Point::new(0, 0), Size::new(100, 100))
        .into_styled(PrimitiveStyle::with_fill(Rgb888::RED))
        .draw(&mut framebuffer_target)
        .unwrap();

    let style = PrimitiveStyleBuilder::new()
        .stroke_color(Rgb888::RED)
        .stroke_width(3)
        .fill_color(Rgb888::WHITE)
        .build();

    let fb_width = framebuffer_target.width as f32;
    let fb_height = framebuffer_target.height as f32;
    for i in 0..5 {
        let x = (fb_width / 9.0) * (i as f32 + 1.0) - 10.0;
        let y = (fb_height / 9.0) * (i as f32 + 1.0);
        let radius = 10.0 + i as f32 * 2.5;

        Circle::new(Point::new(x as i32, y as i32), radius as u32)
            .into_styled(style)
            .draw(&mut framebuffer_target)
            .unwrap();
    }

    let mut theophe = Theophe::new(framebuffer_target);
    theophe.write_line("Welcome to Theophe");
    // theophe.write_str("agrwinonnnononononono nononononononononononooogowniognewagiowe gagrwinonnnonononononononononon ononononononooogowniognewagiowegagrwinonnnonon ononononononononononononononooogowniognewagio");
    // write!(
    //     theophe,
    //     "The current framebuffer size is {}x{}",
    //     fb_width, fb_height
    // )
    // .unwrap();
    // write!(theophe, "aFASFASfASF {}\n", fb_width).unwrap();
    // write!(theophe, "arewhrehrehaerhre {} ", fb_width).unwrap();
    // write!(theophe, "ahrehearhearhaheerh {}", fb_width).unwrap();

    theophe.render();

    loop {
        hlt();
    }

    exit_qemu(QemuExitCode::Success);
}
