#![no_std]
#![no_main]

mod boot;

use core::arch::asm;
use core::fmt::Write;
use embedded_graphics::prelude::*;
use kernel::data_structures::vector::Vec;
use kernel::{
    allocator, graphics::framebuffer::FrameBufferTarget, programs::theophe::Theophe, serial_print,
    serial_println,
};
use x86_64::{instructions::hlt, structures::paging::frame};
extern crate alloc;
use alloc::string::String;

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

fn test_process_system() {
    use kernel::process::{
        process_manager::PROCESS_MANAGER,
        syscall::{SyscallError, SystemCall, handle_syscall}
    };

    // Check arche
    {
        let pm = PROCESS_MANAGER.lock();
        if let Ok(arche) = pm.get_process(0) {
            serial_println!("Arche PID: {}, Name: {}", arche.pid, arche.name);
        }
    }

    let process_name = "process_1";
    let process_2_name = "process_2";

    let result1 = handle_syscall(
        0,
        SystemCall::CreateProcess {
            parent_pid: 0,
            name_ptr: process_name.as_ptr(),
            name_len: process_name.len() as u8,
            is_out: false,
        },
    );

    let result2 = handle_syscall(
        0,
        SystemCall::CreateProcess {
            parent_pid: 0,
            name_ptr: process_2_name.as_ptr(),
            name_len: process_2_name.len() as u8,
            is_out: false,
        },
    );

    if result1.is_ok() && result2.is_ok() {
        {
            let pm = PROCESS_MANAGER.lock();
            if let Ok(p1) = pm.get_process(1) {
                serial_println!(
                    "P1 - PID: {}, Name: {}, Parent: {}",
                    p1.pid,
                    p1.name,
                    p1.parent_pid
                );
                serial_println!("P1 - Memory limit: {}", p1.resources.memory_limit);
            }
            if let Ok(p2) = pm.get_process(2) {
                serial_println!(
                    "P2 - PID: {}, Name: {}, Parent: {}",
                    p2.pid,
                    p2.name,
                    p2.parent_pid
                );
                serial_println!("P2 - Memory limit: {}", p2.resources.memory_limit);
            }
        }

        let _ = handle_syscall(
            1,
            SystemCall::TerminateProcess {
                pid_to_kill: 1,
                exit_code: 0,
                kill_children: false,
            },
        );

        {
            let pm = PROCESS_MANAGER.lock();
            if let Ok(p1) = pm.get_process(1) {
                serial_println!("Process 1 after termination:");
                serial_println!("State: {:?}", p1.state);
                serial_println!("Exit code: {:?}", p1.exit_code);
            }
        }
    } else {
        serial_println!("Failed to create processes");
    }
}

fn main() -> ! {
    serial_println!("Welcome to MofuOS!");

    test_process_system();

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

    let mut vec = Vec::<i32>::with_capacity(4);
    vec.push(1);
    vec.push(2);
    vec.push(3);
    vec.push(1);
    vec.push(2);
    vec.push(3);
    for i in 0..vec.size {
        serial_println!("vec[{}] = {}", i, vec.get(i));
    }
    serial_println!("Vector capacity: {}", vec.capacity);

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
