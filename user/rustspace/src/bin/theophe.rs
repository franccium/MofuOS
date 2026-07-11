#![no_std]
#![no_main]
#![feature(portable_simd)]

extern crate alloc;
extern crate rustspace;

use alloc::format;
use core::arch::global_asm;
use core::cmp::min;
use core::fmt::Write;
use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyle, ascii::FONT_8X13},
    pixelcolor::Rgb888,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, LineHeight, Text, TextStyle, TextStyleBuilder},
};
use rustspace::gfx::surface::UserSurface;

global_asm!(
    ".section .text.entry",
    ".global _start",
    "_start:",
    "    call main",
    "    ud2",
);

// --- Theophe terminal renderer ---

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
    pub draw_target: D,
    curr_line_idx: usize,
    max_chars_per_line: usize,
    lines: [Line; MAX_LINES],
}

impl<D: DrawTarget<Color = Rgb888>> Theophe<D> {
    pub fn new(draw_target: D) -> Self {
        let bounding_box = draw_target.bounding_box();
        let max_chars_per_line = (bounding_box.size.width / CHARACTER_WIDTH as u32) as usize;
        Self {
            draw_target,
            curr_line_idx: 0,
            max_chars_per_line,
            lines: [Line::new(); MAX_LINES],
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

    fn write_bytes(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut start = 0;
        for i in 0..bytes.len() {
            if bytes[i] == b'\n' {
                if i > start {
                    self.append_bytes(&bytes[start..i]);
                }
                self.newline();
                start = i + 1;
            }
        }
        if start < bytes.len() {
            self.append_bytes(&bytes[start..]);
        }
    }

    pub fn write_line(&mut self, text: &str) {
        self.write_bytes(text);
        self.newline();
    }

    pub fn write_str(&mut self, text: &str) {
        self.write_bytes(text);
    }

    fn newline(&mut self) {
        if self.curr_line_idx < MAX_LINES - 1 {
            self.curr_line_idx += 1;
        } else {
            for i in 1..MAX_LINES {
                self.lines[i - 1] = core::mem::replace(&mut self.lines[i], Line::new());
            }
        }
    }

    pub fn clear(&mut self) {
        self.curr_line_idx = 0;
        for line in &mut self.lines {
            line.clear();
        }
        self.clear_screen();
    }

    fn clear_screen(&mut self) {
        let terminal_height = (MAX_LINES * CHARACTER_HEIGHT
            + (MAX_LINES - 1) * (LINE_SPACING as usize - CHARACTER_HEIGHT))
            as u32;
        let _ = Rectangle::new(
            Point::new(0, 0),
            Size::new(self.draw_target.bounding_box().size.width, terminal_height),
        )
        .into_styled(PrimitiveStyle::with_fill(BACKGROUND_COLOR))
        .draw(&mut self.draw_target);
    }

    fn redraw_all(&mut self) {
        //TODO: clear takes a LONG time, compositor has clears figured out
        self.clear_screen();
        rustspace::println!("theophe: redraw_all - begin");
        for i in 0..=self.curr_line_idx {
            if !self.lines[i].is_empty() {
                let _ = Text::with_text_style(
                    self.lines[i].as_str(),
                    Point::new(MARGIN_LEFT, MARGIN_TOP + i as i32 * LINE_SPACING),
                    CHARACTER_STYLE,
                    TEXT_STYLE,
                )
                .draw(&mut self.draw_target);

                rustspace::println!(
                    "theophe: redraw_all - line {}: {}",
                    i,
                    self.lines[i].as_str()
                );
            }
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
    let mut terminal = Theophe::new(surface);

    rustspace::println!("theophe: writing");

    terminal.write_line("Theophe");
    terminal.write_line("=======================");

    rustspace::println!("theophe: starting loop");

    unsafe { rustspace::sys_yield() };

    let mut frame: u32 = 0;
    loop {
        // Write a frame counter line every 256 frames to show it's alive
        // if frame & 0xFF == 0 {
        let msg = format!("frame {}", frame);
        terminal.write_line(&msg);
        // }

        rustspace::println!("theophe: loop - begin");

        terminal.render();
        unsafe { rustspace::sys_present_window(window_id) };
        unsafe {
            terminal.draw_target.swap();
        }
        //unsafe { rustspace::sys_yield() };

        frame = frame.wrapping_add(1);
        rustspace::println!("theophe: loop - end");
    }

    unsafe { rustspace::sys_exit(0) };
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    rustspace::println!("theophe: panic");
    unsafe { rustspace::sys_exit(1) }
}
