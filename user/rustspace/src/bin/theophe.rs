#![no_std]
#![no_main]
#![feature(portable_simd)]

extern crate alloc;
extern crate rustspace;

use alloc::{format, vec::Vec};
use core::any::Any;
use core::cmp::min;
use core::fmt::Write;
use core::{arch::global_asm, sync::atomic::Ordering};
use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyle, ascii::FONT_8X13},
    pixelcolor::Rgb888,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, LineHeight, Text, TextStyle, TextStyleBuilder},
};
use rustspace::{
    AsciiChar, EVENT_BUFFER_ADDR, EventReader, EventType, InputEvent, KeyState, Keys,
    gfx::{color::Rgba8888UNORM, surface::UserSurface},
};
use rustspace::{KeyCode, WindowInfo, sys_get_window_info};

const DEBUG_LOGS: bool = false;
macro_rules! serial_println {
    ($($arg:tt)*) => {
        if DEBUG_LOGS {
            rustspace::println!($($arg)*);
        }
    };
}

global_asm!(
    ".section .text.entry",
    ".global _start",
    "_start:",
    "    call main",
    "    ud2",
);

const CHARACTER_WIDTH: usize = 8;
const CHARACTER_HEIGHT: usize = 13;
const MARGIN_LEFT: i32 = 4;
const MARGIN_TOP: i32 = 4;
const MAX_LINES: usize = 20;
const LINE_SPACING: i32 = 15;
const MAX_CHARS_PER_LINE: usize = 80;

const CHARACTER_STYLE: MonoTextStyle<Rgb888> = MonoTextStyle::new(&FONT_8X13, Rgb888::WHITE);
const BACKGROUND_COLOR: Rgb888 = Rgb888::BLACK;

const TEXT_STYLE: TextStyle = TextStyleBuilder::new()
    .alignment(Alignment::Left)
    .line_height(LineHeight::Percent(150))
    .build();

#[derive(Clone, Copy)]
struct Line {
    chars: [u8; MAX_CHARS_PER_LINE],
    length: usize,
}

impl Line {
    const fn new() -> Self {
        Self {
            chars: [0; MAX_CHARS_PER_LINE],
            length: 0,
        }
    }

    fn clear(&mut self) {
        self.length = 0;
    }
    fn is_empty(&self) -> bool {
        self.length == 0
    }

    fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.chars[..self.length]) }
    }

    fn write_slice(&mut self, slice: &[u8]) -> usize {
        let n = min(slice.len(), MAX_CHARS_PER_LINE - self.length);
        self.chars[self.length..self.length + n].copy_from_slice(&slice[..n]);
        self.length += n;
        n
    }
}

pub struct Theophe<D: DrawTarget<Color = Rgb888>> {
    needs_redraw: bool,
    window_id: u32,
    curr_line_idx: usize,
    max_chars_per_line: usize,
    last_command: Line,
    pub draw_target: D,
    lines: [Line; MAX_LINES],
}

impl<D: DrawTarget<Color = Rgb888>> Theophe<D> {
    pub fn new(draw_target: D, window_id: u32) -> Self {
        let bounding_box = draw_target.bounding_box();
        let max_chars_per_line = (bounding_box.size.width / CHARACTER_WIDTH as u32) as usize;
        Self {
            needs_redraw: true,
            window_id,
            draw_target,
            curr_line_idx: 0,
            max_chars_per_line,
            lines: [Line::new(); MAX_LINES],
            last_command: Line::new(),
        }
    }

    pub fn render(&mut self) {
        self.redraw_all();
    }

    fn get_last_line(&mut self) -> &mut Line {
        if self.lines[self.curr_line_idx].length < self.max_chars_per_line {
            &mut self.lines[self.curr_line_idx]
        } else {
            self.curr_line_idx = min(self.curr_line_idx + 1, MAX_LINES - 1);
            &mut self.lines[self.curr_line_idx]
        }
    }

    fn append_bytes(&mut self, bytes: &[u8]) {
        let mut start = 0;
        while start < bytes.len() {
            let remaining = bytes.len() - start;
            let max_per_line = self.max_chars_per_line;
            let line = self.get_last_line();
            let space = max_per_line - line.length;

            if remaining <= space {
                line.write_slice(&bytes[start..]);
                break;
            }

            // find word-break
            let mut split = min(space, remaining);
            for i in (0..split).rev() {
                if bytes[start + i] == b' ' {
                    split = i + 1;
                    break;
                }
            }
            if split == 0 {
                split = space;
            }

            let written = {
                let line = self.get_last_line();
                line.write_slice(&bytes[start..start + split])
            };
            self.newline();
            start += written;
        }
    }

    fn _write_bytes(&mut self, bytes: &[u8]) {
        let mut bytes_start = 0;
        let bytes_len = bytes.len();
        let max_chars_per_line = self.max_chars_per_line;

        for i in 0..bytes_len {
            if bytes[i] == b'\n' && i > bytes_start {
                let line = self.get_last_line();
                let written = line.write_slice(&bytes[bytes_start..i]);
                serial_println!(
                    "Found newline, written: {}, space left now: {}",
                    written,
                    max_chars_per_line - line.length
                );
            }
        }

        while bytes_start < bytes_len {
            let remaining = bytes_len - bytes_start;
            let line = self.get_last_line();
            let space_left = max_chars_per_line - line.length;
            serial_println!("Remaining bytes: {}", remaining);

            if remaining <= space_left {
                let written = line.write_slice(&bytes[bytes_start..]);
                serial_println!(
                    "Fit in last line, written: {}, space left now: {}",
                    written,
                    max_chars_per_line - line.length
                );
                assert!(written == remaining);
                break;
            } else {
                //let line_start = line.length;

                // Find a good breaking point (a space)
                let mut split_point = min(space_left, remaining);
                for i in (0..split_point).rev() {
                    if bytes[bytes_start + i] == b' ' {
                        split_point = i + 1; // Include the space
                        break;
                    }
                }

                // If no space found, split at line end
                if split_point == 0 {
                    split_point = space_left;
                }

                let slice = &bytes[bytes_start..bytes_start + split_point];

                let line = self.get_last_line();

                let written = line.write_slice(slice);
                self.newline();
                bytes_start += written;
            }
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        let bytes_len = bytes.len();
        let mut bytes_start = 0;

        for i in 0..bytes_len {
            if bytes[i] == b'\n' {
                if i > bytes_start {
                    self._write_bytes(&bytes[bytes_start..i]);
                }
                self.newline();
                bytes_start = i + 1;
            }
        }

        if bytes_start < bytes_len {
            self._write_bytes(&bytes[bytes_start..]);
        }

        self.needs_redraw = true;
    }

    pub fn write_line(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
        self.newline();
    }

    pub fn write_str(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    fn newline(&mut self) {
        if self.curr_line_idx < MAX_LINES - 1 {
            self.curr_line_idx += 1;
        } else {
            for i in 1..MAX_LINES {
                self.lines[i - 1] = core::mem::replace(&mut self.lines[i], Line::new());
            }
        }
        self.needs_redraw = true;
    }

    pub fn clear(&mut self) {
        self.curr_line_idx = 0;
        for line in &mut self.lines {
            line.clear();
        }
        self.clear_screen();

        self.needs_redraw = true;
    }

    fn clear_screen(&mut self) {
        let surface = unsafe { &mut *(&mut self.draw_target as *mut D as *mut UserSurface) };
        surface.clear(Rgba8888UNORM::BLACK);
    }

    fn redraw_all(&mut self) {
        self.clear_screen();
        serial_println!("theophe: redraw_all - begin");
        for i in 0..=self.curr_line_idx {
            if !self.lines[i].is_empty() {
                let _ = Text::with_text_style(
                    self.lines[i].as_str(),
                    Point::new(MARGIN_LEFT, MARGIN_TOP + i as i32 * LINE_SPACING),
                    CHARACTER_STYLE,
                    TEXT_STYLE,
                )
                .draw(&mut self.draw_target);

                serial_println!(
                    "theophe: redraw_all - line {}: {}",
                    i,
                    self.lines[i].as_str()
                );
            }
        }
    }

    fn recall_last_command(&mut self) {
        if !self.last_command.is_empty() {
            self.lines[self.curr_line_idx] = self.last_command;
        }
    }

    fn backspace(&mut self) {
        let line = &mut self.lines[self.curr_line_idx];
        if line.length > 0 {
            line.length -= 1;
        }
    }

    fn execute_command(&mut self, line: &Line) {
        let s = line.as_str().trim();
        if s.is_empty() {
            return;
        }

        let (cmd, args) = match s.find(' ') {
            Some(i) => s.split_at(i),
            None => (s, ""),
        };
        let args = args.trim_matches(' ');

        match cmd {
            "deb" => {
                self.write_line("deb!");
            }
            "odys" => {
                // TODO: parse starting directory and pass to odys
                unsafe {
                    match rustspace::lookup_program(cmd) {
                        Some(elf_path) => {
                            let chars: Vec<char> = elf_path.chars().collect();
                            let name = "odys";
                            let name_chars = name.chars().collect::<Vec<char>>();
                            let args = [0u8; 0];
                            rustspace::sys_create_process(
                                chars.as_slice(),
                                name_chars.as_slice(),
                                &args,
                            );
                        }
                        None => {
                            self.write_line("Command not found");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_special_key(&mut self, key: KeyCode, key_state: KeyState) {
        match key {
            KeyCode::ArrowUp => {
                self.recall_last_command();
                self.needs_redraw = true;
            }
            KeyCode::ArrowLeft => {}
            KeyCode::ArrowDown => {}
            _ => {}
        }
    }

    pub fn handle_event(&mut self, event: InputEvent) {
        let v = event.value;
        match event.event_type {
            EventType::CharEvent => {
                if let Some(c) = char::from_u32(v) {
                    match c {
                        AsciiChar::BACKSPACE => self.backspace(),
                        AsciiChar::NEWLINE | AsciiChar::CARRIAGE_RETURN => {
                            self.last_command = self.lines[self.curr_line_idx];
                            let cmd = self.last_command;
                            self.newline();
                            self.execute_command(&cmd);
                        }
                        c if !c.is_control() => {
                            self.write_bytes(&[c as u8]);
                        }
                        _ => {}
                    }
                    self.needs_redraw = true;
                }
            }
            EventType::KeyEvent => {
                let keycode = unsafe { core::mem::transmute::<u8, KeyCode>(event.value as u8) };
                let key_state = unsafe { core::mem::transmute::<u8, KeyState>(event.extra as u8) };
                self.handle_special_key(keycode, key_state);
            }
            EventType::MouseEvent => {
                let mouse_event = event.decode_mouse();
            }
            _ => {}
        }
    }
}

impl<D: DrawTarget<Color = Rgb888>> Write for Theophe<D> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        Theophe::write_str(self, s);
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    let window_id = unsafe { rustspace::sys_create_window(600, 400, 50, 50) };
    if window_id == u32::MAX {
        rustspace::println!("theophe: create_window failed");
        unsafe { rustspace::sys_exit(1) }
    }
    rustspace::println!("theophe: window id={}", window_id);

    let pixels = unsafe { rustspace::sys_map_window_buffer(window_id) };
    if pixels.is_null() {
        rustspace::println!("theophe: map_window_buffer failed");
        unsafe { rustspace::sys_exit(1) }
    }
    let pixels_second = ((pixels as u64) + 8 * 1024 * 1024) as *mut u32;

    let (width, height) = unsafe { rustspace::sys_get_window_size(window_id) };
    if width == 0 || height == 0 {
        rustspace::println!("theophe: get_window_size failed");
        unsafe { rustspace::sys_exit(1) }
    }

    rustspace::println!(
        "theophe: window {}x{} id={} mapped at {:p}, front: {:p}",
        width,
        height,
        window_id,
        pixels,
        pixels_second
    );

    unsafe { rustspace::syscall1(rustspace::SYS_FOCUS_WINDOW, window_id as u64) };

    let surface = unsafe { UserSurface::new(pixels, pixels_second, width, height) };
    let mut theophe = Theophe::new(surface, window_id);

    rustspace::println!("theophe: writing");

    theophe.write_line("Theophe");
    theophe.write_line("=======================");

    rustspace::println!("theophe: starting loop");

    rustspace::init_programs();

    let mut window_info = WindowInfo::zeroed();
    let ok = unsafe { sys_get_window_info(window_id, &mut window_info) };
    rustspace::println!(
        "theophe: window info: {}x{} event_buffer_vaddr: {}",
        window_info.width,
        window_info.height,
        window_info.event_buffer_vaddr,
    );

    //let mut event_reader = unsafe { EventReader::new(window_info.event_buffer_vaddr as usize) };
    let mut event_reader = unsafe { EventReader::new(EVENT_BUFFER_ADDR) };

    unsafe { rustspace::sys_yield() };

    let mut frame: u32 = 0;
    loop {
        loop {
            // let buffer = unsafe {
            //     &*(rustspace::PROGRAM_SHARED_DATA_ADDR as *const rustspace::ProgramSharedDataBuffer)
            // };
            // let focused_window_id = buffer.focused_window_id.load(Ordering::Acquire);
            // if theophe.window_id != focused_window_id {
            //     rustspace::println!(
            //         "theophe: focused_window_id: {}, this id: {}",
            //         focused_window_id,
            //         theophe.window_id
            //     );
            //     break;
            // }

            let event = event_reader.try_read(window_id);
            match event {
                Some(event) => {
                    theophe.handle_event(event);
                }
                None => break,
            }
        }

        // if frame & 0xF == 0 {
        // let msg = format!("frame {}", frame);
        // theophe.write_line(&msg);
        // }

        //rustspace::println!("theophe: loop - begin");

        let backbuffer_redraw_required = theophe.needs_redraw;
        if theophe.needs_redraw {
            theophe.render();
            theophe.needs_redraw = false;
        }
        unsafe { rustspace::sys_present_window(window_id) };
        unsafe {
            theophe.draw_target.swap();
            //NOTE: we also need to redraw the second buffer, so one more redrawing frame is required
            theophe.needs_redraw = backbuffer_redraw_required;
        }
        //unsafe { rustspace::sys_yield() };

        frame = frame.wrapping_add(1);
        // unsafe {
        //     rustspace::sys_yield();
        // }
        //rustspace::println!("theophe: loop - end");
    }

    unsafe { rustspace::sys_exit(0) };
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    rustspace::println!("theophe: panic");
    unsafe { rustspace::sys_exit(1) }
}
