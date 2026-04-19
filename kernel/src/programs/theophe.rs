use crate::graphics::framebuffer::FrameBufferTarget;
use crate::serial_println;

use core::cmp::min;
use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_8X13},
    pixelcolor::Rgb888,
    prelude::*,
    text::{Alignment, LineHeight, Text, TextStyle, TextStyleBuilder},
};

const CHARACTER_WIDTH: usize = 8; // FONT_8X13 width
const CHARACTER_HEIGHT: usize = 13;
const MARGIN_LEFT: i32 = 20;
const MARGIN_TOP: i32 = 30;
const MAX_LINES: usize = 20;
const LINE_SPACING: i32 = 15;
const MAX_CHARS_PER_LINE: usize = 80;

const CHARACTER_STYLE: MonoTextStyle<Rgb888> = MonoTextStyle::new(&FONT_8X13, Rgb888::WHITE);
const BACKGROUND_COLOR: Rgb888 = Rgb888::BLACK;

const TEXT_STYLE: TextStyle = TextStyleBuilder::new()
    .alignment(Alignment::Left)
    .line_height(LineHeight::Percent(150))
    .build();

//TODO: do something more efficient instead of a line buffer, for now we have this just for simplicity
// also this may be good cause i can just render it with embedded-graphics crate
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
        let copied_len = min(slice.len(), MAX_CHARS_PER_LINE - self.length);
        self.chars[self.length..self.length + copied_len].copy_from_slice(&slice[..copied_len]);
        self.length += copied_len;
        copied_len
    }
}

//TODO: should add buffering, and not clear the whole screen for each render
//TODO: <'a> for now, when we get a compositor it will have its own buffer
pub struct Theophe<'a> {
    framebuffer_target: FrameBufferTarget<'a>,
    curr_line_idx: usize,
    lines: [Line; MAX_LINES],
}

impl<'a> Theophe<'a> {
    pub fn new(framebuffer_target: FrameBufferTarget<'a>) -> Self {
        Self {
            framebuffer_target,
            curr_line_idx: 0,
            lines: [Line::new(); MAX_LINES],
        }
    }

    pub fn render(&mut self) {
        self.redraw_all();
    }

    fn get_last_line(&mut self) -> &mut Line {
        if self.lines[self.curr_line_idx].length < MAX_CHARS_PER_LINE {
            &mut self.lines[self.curr_line_idx]
        } else {
            self.curr_line_idx = min(self.curr_line_idx + 1, MAX_LINES - 1);
            &mut self.lines[self.curr_line_idx]
        }
    }

    fn _write_bytes(&mut self, bytes: &[u8]) {
        let mut bytes_start = 0;
        let bytes_len = bytes.len();

        for i in 0..bytes_len {
            if bytes[i] == b'\n' {
                if i > bytes_start {
                    let line = self.get_last_line();
                    let written = line.write_slice(&bytes[bytes_start..i]);
                    serial_println!(
                        "Found newline, written: {}, space left now: {}",
                        written,
                        MAX_CHARS_PER_LINE - line.length
                    );
                }
            }
        }

        while bytes_start < bytes_len {
            let remaining = bytes_len - bytes_start;
            let line = self.get_last_line();
            let space_left = MAX_CHARS_PER_LINE - line.length;
            serial_println!("Remaining bytes: {}", remaining);

            if remaining <= space_left {
                let written = line.write_slice(&bytes[bytes_start..]);
                serial_println!(
                    "Fit in last line, written: {}, space left now: {}",
                    written,
                    MAX_CHARS_PER_LINE - line.length
                );
                assert!(written == remaining);
                break;
            } else {
                //let line_start = line.length;

                // Find a good breaking point (a space)
                let mut split_point = MAX_CHARS_PER_LINE;
                let search_end = min(space_left, remaining);
                for i in (0..search_end).rev() {
                    if bytes[bytes_start + i] == b' ' {
                        split_point = i + 1; // Include the space
                        break;
                    }
                }

                let slice = &bytes[bytes_start..bytes_start + split_point];

                let line = self.get_last_line();

                let written = line.write_slice(slice);
                self.newline();
                bytes_start += written;
            }
        }
    }

    fn write_bytes(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let bytes_len = bytes.len();
        let mut bytes_start = 0;

        for i in 0..bytes_len {
            if bytes[i] == b'\n' {
                serial_println!(
                    "\\n detected: Writing: '{}'; bytes_length: {}, curr_line_idx: {}",
                    text,
                    bytes_len,
                    self.curr_line_idx
                );
                if i > bytes_start {
                    self._write_bytes(&bytes[bytes_start..i]);
                }
                self.newline();
                bytes_start = i + 1;
            }
        }

        if bytes_start < bytes_len {
            serial_println!(
                "Writing: '{}'; bytes_length: {}, curr_line_idx: {}",
                text,
                bytes_len,
                self.curr_line_idx
            );
            self._write_bytes(&bytes[bytes_start..]);
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
        let clear_rect = embedded_graphics::primitives::Rectangle::new(
            Point::new(MARGIN_LEFT, MARGIN_TOP),
            Size::new(
                (MAX_CHARS_PER_LINE * CHARACTER_WIDTH) as u32,
                terminal_height,
            ),
        );

        let _ = clear_rect
            .into_styled(embedded_graphics::primitives::PrimitiveStyle::with_fill(
                BACKGROUND_COLOR,
            ))
            .draw(&mut self.framebuffer_target);
    }

    fn redraw_all(&mut self) {
        self.clear_screen();

        serial_println!("Theophe: redraw_all");

        for i in 0..=self.curr_line_idx {
            if !self.lines[i].is_empty() {
                let _ = Text::with_text_style(
                    self.lines[i].as_str(),
                    self.get_pos(i),
                    CHARACTER_STYLE,
                    TEXT_STYLE,
                )
                .draw(&mut self.framebuffer_target);
            }
        }
    }

    fn get_pos(&self, line_index: usize) -> Point {
        Point::new(MARGIN_LEFT, MARGIN_TOP + (line_index as i32 * LINE_SPACING))
    }
}

impl<'a> core::fmt::Write for Theophe<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_str(s);
        Ok(())
    }
}
