#![no_std]
#![no_main]

mod boot;

use embedded_graphics::prelude::*;
use kernel::data_structures::vector::Vec;
use kernel::process::{ElfLoadError, ElfLoadInfo, elf_loader};
use kernel::{
    filesystem::sirius::{FileType},
    graphics::framebuffer::FrameBufferTarget,
    programs::theophe::Theophe,
    serial_println,
};
use x86_64::{instructions::hlt};
extern crate alloc;

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
        syscall::{SystemCall, handle_syscall},
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

fn test_filesystem_system() {
    use kernel::filesystem::{
        fat32::test_data::create_fat32_image, init_filesystem, sirius::get_sirius,
    };

    serial_println!("\nTesting Filesystem");

    let fat32_image_data = create_fat32_image();

    let image_slice = &*fat32_image_data;

    match init_filesystem(image_slice) {
        Ok(_) => {
            serial_println!("Filesystem initialized");

            // Try to read root directory
            {
                let mut sirius = get_sirius();
                match sirius.list_directory("/") {
                    Ok(entries) => {
                        serial_println!("Root directory opened");
                        serial_println!("  Found {} entries:", entries.len());
                        for entry in &entries {
                            serial_println!("   - {} ({} bytes)", entry.name, entry.size);

                            if entry.file_type == FileType::File {
                                let mut buffer = [0u8; 64];
                                match sirius.read_file(&entry.name, 0, &mut buffer) {
                                    Ok(contents) => {
                                        serial_println!(
                                            "    Read file contents: '{}' {}",
                                            contents,
                                            entry.size
                                        );
                                        let content_str = core::str::from_utf8(&buffer[..contents])
                                            .unwrap_or("not utf8?");
                                        serial_println!("    Content: '{}'", content_str);
                                    }
                                    Err(e) => {
                                        serial_println!("    Failed to read file: {:?}", e);
                                    }
                                }
                            }
                        }

                        serial_println!("FILE CREATION");
                        match sirius.create_file("/newfile.txt") {
                            Ok(node) => {
                                serial_println!("Created new file: {}", node.name);
                            }
                            Err(e) => {
                                serial_println!("Failed to create file: {:?}", e);
                            }
                        }

                        serial_println!("DIRECTORY CREATION");
                        match sirius.create_directory("/somedir") {
                            Ok(node) => {
                                serial_println!("Created new directory: {}", node.name);
                            }
                            Err(e) => {
                                serial_println!("Failed to create directory: {:?}", e);
                            }
                        }

                        serial_println!("FILE IN SUBDIR CREATION");
                        match sirius.create_file("/somedir/nested.txt") {
                            Ok(node) => {
                                serial_println!("Created new file: {}", node.name);
                            }
                            Err(e) => {
                                serial_println!("Failed to create file: {:?}", e);
                            }
                        }
                    }
                    Err(e) => serial_println!("Failed to open root: {:?}", e),
                }
            }

            {
                let mut sirius = get_sirius();
                match sirius.list_directory("/") {
                    Ok(entries) => {
                        serial_println!("Root directory opened");
                        serial_println!("  Found {} entries:", entries.len());
                        for entry in &entries {
                            serial_println!("   - {} ({} bytes)", entry.name, entry.size);

                            if entry.file_type == FileType::File {
                                let mut buffer = [0u8; 64];
                                match sirius.read_file(&entry.name, 0, &mut buffer) {
                                    Ok(contents) => {
                                        serial_println!(
                                            "    Read file contents: '{}' {}",
                                            contents,
                                            entry.size
                                        );
                                        let content_str = core::str::from_utf8(&buffer[..contents])
                                            .unwrap_or("not utf8?");
                                        serial_println!("    Content: '{}'", content_str);
                                    }
                                    Err(e) => {
                                        serial_println!("    Failed to read file: {:?}", e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => serial_println!("Failed to open root: {:?}", e),
                }

                serial_println!("FILE DELETION");
                match sirius.delete("/newfile.txt") {
                    Ok(_) => {
                        serial_println!("  deleted");
                    }
                    Err(e) => {
                        serial_println!("  Failed to delete file: {:?}", e);
                    }
                }
                match sirius.delete("/somedir") {
                    Ok(_) => {
                        serial_println!("  deleted");
                    }
                    Err(e) => {
                        serial_println!("  Failed to delete file: {:?}", e);
                    }
                }
                match sirius.list_directory("/") {
                    Ok(entries) => {
                        serial_println!("Root directory opened");
                        serial_println!("  Found {} entries:", entries.len());
                        for entry in &entries {
                            serial_println!("   - {} ({} bytes)", entry.name, entry.size);

                            if entry.file_type == FileType::File {
                                let mut buffer = [0u8; 64];
                                match sirius.read_file(&entry.name, 0, &mut buffer) {
                                    Ok(contents) => {
                                        serial_println!(
                                            "    Read file contents: '{}' {}",
                                            contents,
                                            entry.size
                                        );
                                        let content_str = core::str::from_utf8(&buffer[..contents])
                                            .unwrap_or("not utf8?");
                                        serial_println!("    Content: '{}'", content_str);
                                    }
                                    Err(e) => {
                                        serial_println!("    Failed to read file: {:?}", e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => serial_println!("Failed to open root: {:?}", e),
                }
            }
        }
        Err(e) => serial_println!("Failed to initialize filesystem: {}", e),
    }
}

fn main() -> ! {
    serial_println!("Welcome to MofuOS!");

    kernel::process::syscall::init_syscall_stack();

    match ElfLoadInfo::from_elf_data(&elf_loader::TEST_ELF) {
        Err(ElfLoadError::ParseError(e)) => {
            serial_println!("Error loading elf: ParseError: {:?}", e)
        }
        Err(ElfLoadError::InvalidMagic) => {
            serial_println!("Error loading elf: InvalidMagic")
        }
        Err(ElfLoadError::InvalidArch) => {
            serial_println!("Error loading elf: InvalidArch")
        }
        Err(ElfLoadError::InvalidHeader) => {
            serial_println!("Error loading elf: InvalidHeader")
        }
        Err(kernel::process::ElfLoadError::InvalidType) | Err(kernel::process::ElfLoadError::NoLoadableSegments) | Err(kernel::process::ElfLoadError::ReadError) => todo!(),
        Ok(info) => {
            serial_println!("Loaded elf info: {:?}", info.entry_point)
        }
    }

    //kernel::process_start::create_init_process();
    //kernel::process_start::create_and_run_init_process();

    //test_process_system();
    //test_filesystem_system();

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

    // let mut theophe = Theophe::new(framebuffer_target);
    // theophe.write_line("");
    // theophe.write_line("  hi");
    // theophe.write_line("==========================================================");
    // let cpu_info = kernel::util::cpuinfo::get_cpu_info();
    // let cpu_info_str = cpu_info.to_pretty_string();
    // theophe.write_str(&cpu_info_str);

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

    // theophe.render();

    use kernel::graphics::color::{Rgba8888UNORM, rgba_to_xrgb};
    use kernel::graphics::window::{Window, WindowBuffer};
    use kernel::graphics::compositor::Compositor;
    //TODO: compositor should own the framebuffer; adjust theophe to work as other processes would, with its own window backbufer
    serial_println!("Framebuffer size: {}x{}", fb_width, fb_height);
    let mut compositor = Compositor::new(fb_width as u32,  fb_height as u32);
    let (window_id, window_buffer) = compositor.create_window(200, 150, 50, 50);
    serial_println!("Created window with ID: {}", window_id);
    {
        let mut back_buffer = window_buffer.back_buffer_mut();
        for y in 0..150 {
            for x in 0..200 {
                back_buffer.write_pixel(x, y, Rgba8888UNORM::from_rgb_emb(Rgb888::BLUE));
            }
        }

        serial_println!("Presenting window 1 ");
        window_buffer.present();
    }
    let (window2_id, window2_buffer) = compositor.create_window(400, 500, 100, 90);
    compositor.set_z_index(window2_id, 5);
    serial_println!("Created window with ID: {}", window2_id);
    {
        let mut back_buffer = window2_buffer.back_buffer_mut();
        for y in 0..window2_buffer.height {
            for x in 0..window2_buffer.width {
                let r = (x as f32 / window2_buffer.width as f32 * 255.0) as u8;
                let g = (y as f32 / window2_buffer.height as f32 * 255.0) as u8;
                let b = 0;
                back_buffer.write_pixel(x, y, Rgba8888UNORM::from_rgb(r, g, b));
            }
        }

        serial_println!("Presenting window 2 ");
        window2_buffer.present();
    }
    compositor.focus_window(0);
    compositor.compose(&mut framebuffer_target);

    loop {
        hlt();
    }

    exit_qemu(QemuExitCode::Success);
}
