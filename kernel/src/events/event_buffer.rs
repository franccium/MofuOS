use core::sync::atomic::{AtomicU32, Ordering};

use crate::{memory::memory::PAGE_SIZE, serial_println};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyState {
    Pressed = 0,
    Released = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Keys {
    ArrowUp = 0x110000,
    ArrowDown = 0x110001,
    ArrowLeft = 0x110002,
    ArrowRight = 0x110003,
    LeftAlt = 0x120000,
    Backspace = 0x080000,
    Tab = 0x090000,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MouseButtons: u16 {
        const LEFT = 0b0000_0001;
        const RIGHT = 0b0000_0010;
        const MIDDLE = 0b0000_0100;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub x_delta: i16,
    pub y_delta: i16,
    pub buttons: MouseButtons,
    pub x_overflow: bool,
    pub y_overflow: bool,
}
pub struct AsciiChar;
impl AsciiChar {
    pub const BACKSPACE: char = '\x08';
    pub const TAB: char = '\x09';
    pub const NEWLINE: char = '\n';
    pub const CARRIAGE_RETURN: char = '\r';
    pub const ESCAPE: char = '\x1B';
    pub const DELETE: char = '\x7F';
    pub const SPACE: char = ' ';
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EventType {
    None = 0,
    KeyEvent = 1,
    MouseEvent = 2,
}

pub type EventBufferResult<T> = Result<T, EventBufferError>;

#[derive(Debug, Clone, Copy)]
pub enum EventBufferError {
    Full,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct InputEvent {
    pub event_type: EventType,
    pub _pad: [u8; 3],
    pub value: u32,
    pub extra: u32,
    pub reserved: u32,
}

impl InputEvent {
    pub const fn new(event_type: EventType, value: u32, extra: u32) -> Self {
        Self {
            event_type,
            _pad: [0; 3],
            value,
            extra,
            reserved: 0,
        }
    }

    pub fn new_key(keycode: u32, state: KeyState) -> Self {
        Self::new(EventType::KeyEvent, keycode, state as u32)
    }

    pub fn new_mouse(mouse_event: MouseEvent) -> Self {
        let value = mouse_event.x_delta as i32 as u32;
        let mut extra = (mouse_event.y_delta as i16 as u32) & 0xFFFF;
        extra |= ((mouse_event.buttons.bits() as u32) << 16) & 0x00070000;

        let overflow_bits =
            ((mouse_event.x_overflow as u32) << 0) | ((mouse_event.y_overflow as u32) << 1);
        extra |= (overflow_bits << 19) & 0x00180000;

        Self::new(EventType::MouseEvent, value, extra)
    }

    pub fn decode_mouse(&self) -> MouseEvent {
        let x_delta = self.value as i32 as i16;
        let y_delta = (self.extra & 0xFFFF) as i16 as i16;
        let buttons = MouseButtons::from_bits_truncate(((self.extra >> 16) as u16) & 0x07);

        let overflow_bits = (self.extra >> 19) & 0x03;
        let x_overflow = (overflow_bits & 0x01) != 0;
        let y_overflow = (overflow_bits & 0x02) != 0;

        MouseEvent {
            x_delta,
            y_delta,
            buttons,
            x_overflow,
            y_overflow,
        }
    }
}

const BUFFER_HEADER_SIZE: usize = core::mem::size_of::<AtomicU32>() * 3;
const MAX_EVENT_COUNT: usize =
    (PAGE_SIZE - BUFFER_HEADER_SIZE) / core::mem::size_of::<InputEvent>();

#[repr(C, align(4096))]
pub struct EventBuffer {
    pub write_idx: AtomicU32,
    pub read_idx: AtomicU32,
    pub event_count: AtomicU32,

    pub events: [InputEvent; MAX_EVENT_COUNT],
}

impl EventBuffer {
    pub const fn new() -> Self {
        Self {
            write_idx: AtomicU32::new(0),
            read_idx: AtomicU32::new(0),
            event_count: AtomicU32::new(0),
            events: [InputEvent::new(EventType::None, 0, 0); MAX_EVENT_COUNT],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.event_count.load(Ordering::Acquire) == 0
    }

    /// We assume only one event source can push at one time
    pub fn push(&self, event: InputEvent) -> EventBufferResult<()> {
        let event_count = self.event_count.load(Ordering::Acquire);
        if event_count >= MAX_EVENT_COUNT as u32 {
            serial_println!("Event buffer is full, dropping event: {:?}", event);
            return Err(EventBufferError::Full);
        }

        let idx = self.write_idx.load(Ordering::Acquire);
        let event_ptr: *mut InputEvent =
            core::ptr::addr_of!(self.events[idx as usize]) as *mut InputEvent;
        unsafe {
            core::ptr::write_volatile(event_ptr, event);
        }

        core::sync::atomic::fence(Ordering::Release);

        self.write_idx
            .store((idx + 1) % MAX_EVENT_COUNT as u32, Ordering::Release);
        self.event_count.fetch_add(1, Ordering::Release);

        Ok(())
    }

    pub fn read(&self) -> Option<InputEvent> {
        if self.is_empty() {
            return None;
        }

        let idx = self.read_idx.load(Ordering::Acquire);
        let event = self.events[idx as usize];

        core::sync::atomic::fence(Ordering::Acquire);

        self.read_idx
            .store((idx + 1) % MAX_EVENT_COUNT as u32, Ordering::Release);
        self.event_count.fetch_sub(1, Ordering::Release);

        Some(event)
    }
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new()
    }
}
