use crate::events::event_buffer::{
    EventBuffer, EventType, InputEvent, Keys, MouseButtons, MouseEvent,
};
use crate::graphics::FRAMEBUFFER_BYTES_PER_PIXEL;
use crate::graphics::color::{Rgba8888UNORM, rgba_to_xrgb};
use crate::graphics::framebuffer::FrameBufferTarget;
use crate::graphics::window::{INVALID_WINDOW_ID, Window, WindowBuffer, WindowID};
use crate::process::shared_state::get_shared_input_event_buffer;
use crate::{serial_println, serial_println_core};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use embedded_graphics::pixelcolor::Rgb888;
use spin::{Mutex, MutexGuard, Once, RwLock};

static COMPOSITOR: Once<Mutex<Compositor>> = Once::new();

pub fn init_compositor(width: u32, height: u32) {
    COMPOSITOR.call_once(|| Mutex::new(Compositor::new(width, height)));
}

pub fn get_compositor() -> MutexGuard<'static, Compositor> {
    COMPOSITOR.get().expect("compositor not initialized").lock()
}

static INPUT_EVENT_BUFFER: Once<EventBuffer> = Once::new();

pub fn init_input_event_buffer() {
    INPUT_EVENT_BUFFER.call_once(|| EventBuffer::new());
}

pub fn get_input_event_buffer() -> &'static EventBuffer {
    INPUT_EVENT_BUFFER.get().expect("input event buffer not initialized")
}

const NORMALIZE_Z_INDEX_THRESHOLD: u8 = 250;

const MOUSE_DEBUG_PRINT: bool = false;
const CURSOR_COLOR_XRGB: u32 = 0xFFAAAAFF;
const CLEAR_COLOR_XRGB: u32 = 0xFF000000;
const CURSOR_SIZE_X: i32 = 3;
const CURSOR_SIZE_Y: i32 = 10;

pub struct Compositor {
    framebuffer_width: u32,
    framebuffer_height: u32,

    next_window_id: AtomicU32,
    currently_focused_window: Mutex<WindowID>,
    free_window_ids: Mutex<Vec<WindowID>>,
    pub windows: RwLock<Vec<Window>>,

    mouse_x: AtomicI32,
    mouse_y: AtomicI32,
    cursor_visible: AtomicBool,

    mouse_last_drawn_x: i32,
    mouse_last_drawn_y: i32,
}

impl Compositor {
    pub fn new(framebuffer_width: u32, framebuffer_height: u32) -> Self {
        Self {
            framebuffer_width,
            framebuffer_height,
            next_window_id: AtomicU32::new(0),
            currently_focused_window: Mutex::new(0),
            windows: RwLock::new(Vec::new()),
            free_window_ids: Mutex::new(Vec::new()),
            mouse_x: AtomicI32::new(0),
            mouse_y: AtomicI32::new(0),
            cursor_visible: AtomicBool::new(true),
            mouse_last_drawn_x: 0,
            mouse_last_drawn_y: 0,
        }
    }

    pub fn create_window(
        &self,
        width: u32,
        height: u32,
        x: i32,
        y: i32,
    ) -> (WindowID, Arc<WindowBuffer>) {
        let buffer = Arc::new(WindowBuffer::new(width, height, x, y));
        let id = if let Some(free_id) = self.free_window_ids.lock().pop() {
            free_id
        } else {
            self.next_window_id.fetch_add(1, Ordering::Relaxed)
        };

        let window = Window {
            id,
            x,
            y,
            z_index: 0,
            is_visible: true,
            buffer: buffer.clone(),
        };

        let mut windows = self.windows.write();
        if id as usize >= windows.len() {
            windows.push(window);
        } else {
            windows[id as usize] = window;
        }
        windows.sort_by_key(|w| w.z_index);

        *self.currently_focused_window.lock() = id;

        (id, buffer)
    }

    pub fn set_z_index(&self, window_id: WindowID, z_index: u8) {
        if window_id != INVALID_WINDOW_ID {
            let mut windows = self.windows.write();
            let window = windows.get_mut(window_id as usize).unwrap();
            if window.z_index != z_index {
                window.z_index = z_index;
                windows.sort_by_key(|w| w.z_index);
            }
        }
    }

    pub fn focus_window(&self, window_id: WindowID) {
        if window_id != INVALID_WINDOW_ID {
            let mut focused_window = self.currently_focused_window.lock();
            if window_id != *focused_window {
                let mut windows = self.windows.write();
                let max_z_index = windows.first().map(|w| w.z_index).unwrap_or(0);

                let window = windows.get_mut(window_id as usize).unwrap();
                window.z_index = max_z_index + 1;
                *focused_window = window_id;

                if max_z_index > NORMALIZE_Z_INDEX_THRESHOLD {
                    let mut visible: Vec<&mut Window> =
                        windows.iter_mut().filter(|w| w.is_visible).collect();
                    visible.sort_by_key(|w| w.z_index);

                    for (i, window) in visible.iter_mut().enumerate() {
                        window.z_index = i as u8;
                    }
                }
                windows.sort_by_key(|w| w.z_index);
            }
        }
    }

    //TODO: actually do something with the invalid window id markings
    pub fn destroy_window(&self, window_id: WindowID) {
        if window_id != INVALID_WINDOW_ID {
            let mut windows = self.windows.write();
            let window = windows.get_mut(window_id as usize).unwrap();
            window.is_visible = false;
            self.free_window_ids.lock().push(window_id);
        } else {
            serial_println!("Attempted to destroy an invalid window ID: {}", window_id);
        }
    }

    pub fn process_input_events(&mut self) {
        let buffer = get_input_event_buffer();

        while let Some(event) = buffer.read() {
            match event.event_type {
                EventType::MouseEvent => {
                    self.handle_mouse_event(event);
                }
                EventType::KeyEvent => {
                    self.handle_keyboard_event(event);
                }
                EventType::None => {}
            }
        }
    }

    pub fn handle_keyboard_event(&mut self, event: InputEvent) {
        let v = event.value;
        if v == Keys::LeftAlt as u32 {
            serial_println_core!("Compositor intercepted Left Alt")
        }

        let _ = unsafe {
            get_shared_input_event_buffer().push(event);
        };
    }

    fn handle_mouse_event(&mut self, event: InputEvent) {
        let mut mouse_x = self.mouse_x.load(Ordering::Relaxed);
        let mut mouse_y = self.mouse_y.load(Ordering::Relaxed);
        let mouse_event = event.decode_mouse();

        mouse_x += mouse_event.x_delta as i32;
        mouse_y += mouse_event.y_delta as i32;
        mouse_x = mouse_x.clamp(0, self.framebuffer_width as i32 - 1);
        mouse_y = mouse_y.clamp(0, self.framebuffer_height as i32 - 1);

        if MOUSE_DEBUG_PRINT {
            serial_println!(
                "Mouse moved to ({}, {}) with buttons: {:?}",
                mouse_x,
                mouse_y,
                mouse_event.buttons
            );
        }

        self.mouse_x.store(mouse_x, Ordering::Relaxed);
        self.mouse_y.store(mouse_y, Ordering::Relaxed);

        if !mouse_event.buttons.is_empty() {
            self.handle_mouse_click(mouse_x, mouse_y, mouse_event.buttons);
        }

        // TODO: Forward mouse events to the focused window process buffer

        let _ = unsafe {
            get_shared_input_event_buffer().push(event);
        };
    }

    fn handle_mouse_click(&mut self, mouse_x: i32, mouse_y: i32, buttons: MouseButtons) {
        let focused_window_id = *self.currently_focused_window.lock();

        if focused_window_id != INVALID_WINDOW_ID {
            let windows = self.windows.read();
            if let Some(focused_window) = windows.get(focused_window_id as usize) {
                if focused_window.is_visible {
                    let window_x = focused_window.x;
                    let window_y = focused_window.y;
                    let window_width = focused_window.buffer.width as i32;
                    let window_height = focused_window.buffer.height as i32;

                    if mouse_x >= window_x
                        && mouse_x < window_x + window_width
                        && mouse_y >= window_y
                        && mouse_y < window_y + window_height
                    {
                        serial_println!(
                            "Mouse click within focused window {} at ({}, {})",
                            focused_window_id,
                            mouse_x,
                            mouse_y
                        );
                        return;
                    }
                }
            }
        }

        self.find_and_focus_window_at(mouse_x, mouse_y);
    }

    fn find_and_focus_window_at(&self, mouse_x: i32, mouse_y: i32) {
        let windows = self.windows.read();
        let mut candidate_windows: Vec<&Window> = windows
            .iter()
            .filter(|w| w.is_visible)
            .filter(|w| {
                let x = w.x;
                let y = w.y;
                let width = w.buffer.width as i32;
                let height = w.buffer.height as i32;
                mouse_x >= x && mouse_x < x + width && mouse_y >= y && mouse_y < y + height
            })
            .collect();

        if candidate_windows.is_empty() {
            return;
        }

        let top_window = candidate_windows.first().unwrap();

        if let Some(top_window) = candidate_windows.first() {
            if top_window.id != *self.currently_focused_window.lock() {
                self.focus_window(top_window.id);
                serial_println!(
                    "Focused window {} at ({}, {})",
                    top_window.id,
                    mouse_x,
                    mouse_y
                );
            }
        }
    }

    fn draw_cursor(
        &self,
        cursor_x: i32,
        cursor_y: i32,
        color_xrgb: u32,
        framebuffer_ptr: *mut u8,
        framebuffer_pitch: u32,
        framebuffer_width: u32,
        framebuffer_height: u32,
    ) {
        if cursor_x >= 0
            && cursor_x < framebuffer_width as i32
            && cursor_y >= 0
            && cursor_y < framebuffer_height as i32
        {
            let start_x = cursor_x.max(0) as u32;
            let start_y = cursor_y.max(0) as u32;
            let end_x = (cursor_x + CURSOR_SIZE_X).min(framebuffer_width as i32) as u32;
            let end_y = (cursor_y + CURSOR_SIZE_Y).min(framebuffer_height as i32) as u32;

            for y in start_y..end_y {
                for x in start_x..end_x {
                    let dst_offset =
                        (y * framebuffer_pitch as u32 + x * FRAMEBUFFER_BYTES_PER_PIXEL) as usize;

                    unsafe {
                        let dst_ptr = framebuffer_ptr.add(dst_offset).cast::<u32>();
                        core::ptr::write_volatile(dst_ptr, color_xrgb);
                    }
                }
            }
            if MOUSE_DEBUG_PRINT {
                serial_println_core!("Cursor drawn at ({}, {})", cursor_x, cursor_y);
            }
        }
    }

    //TODO:compositor should own the framebuffer
    //TODO: alpha blending
    pub fn compose(&mut self, framebuffer: &mut FrameBufferTarget) {
        let windows = self.windows.read();

        let mut visible_windows: Vec<&Window> = windows.iter().filter(|w| w.is_visible).collect();

        let framebuffer_ptr = framebuffer.address();
        let framebuffer_pitch = framebuffer.pitch;
        let framebuffer_width = framebuffer.width;
        let framebuffer_height = framebuffer.height;

        // clear cursor
        self.draw_cursor(
            self.mouse_last_drawn_x,
            self.mouse_last_drawn_y,
            CLEAR_COLOR_XRGB,
            framebuffer_ptr,
            framebuffer_pitch as u32,
            framebuffer_width as u32,
            framebuffer_height as u32,
        );
        // serial_println!("Composing frame with {} visible windows", visible_windows.len());

        // TODO: clear the framebuffer

        for window in visible_windows {
            if window.is_visible {
                window.buffer.try_swap();

                // serial_println!("Compositing window ID {} at position ({}, {}) with size {}x{}", window.id, window.x, window.y, window.buffer.width, window.buffer.height);

                let start_x = window.x.max(0) as u32;
                let start_y = window.y.max(0) as u32;
                let end_x =
                    (window.x + window.buffer.width as i32).min(framebuffer_width as i32) as u32;
                let end_y =
                    (window.y + window.buffer.height as i32).min(framebuffer_height as i32) as u32;

                if start_x >= end_x || start_y >= end_y {
                    // serial_println!("Skipping window ID {} - out of bounds", window.id);
                    continue;
                }

                let src_x = if window.x > 0 { 0 } else { -window.x as u32 };
                let src_y = if window.y > 0 { 0 } else { -window.y as u32 };

                let copy_width = end_x - start_x;
                let copy_height = end_y - start_y;

                let src_ptr = window.buffer.front_buffer_ptr();

                // serial_println!("Copying window ID {} to framebuffer region ({}, {}) - ({}, {})", window.id, start_x, start_y, end_x, end_y);

                for y in 0..copy_height {
                    let src_offset = ((src_y + y) * window.buffer.width + src_x) as usize;
                    let dst_offset = ((start_y + y) * framebuffer_pitch as u32
                        + start_x * FRAMEBUFFER_BYTES_PER_PIXEL)
                        as usize;

                    unsafe {
                        let dst_ptr = framebuffer_ptr.add(dst_offset).cast::<u32>();
                        core::ptr::copy_nonoverlapping(
                            src_ptr.add(src_offset),
                            dst_ptr,
                            copy_width as usize,
                        );
                    }
                }
            }
        }

        if self.cursor_visible.load(Ordering::Relaxed) {
            let cursor_x = self.mouse_x.load(Ordering::Relaxed);
            let cursor_y = self.mouse_y.load(Ordering::Relaxed);
            self.mouse_last_drawn_x = cursor_x;
            self.mouse_last_drawn_y = cursor_y;

            self.draw_cursor(
                cursor_x,
                cursor_y,
                CURSOR_COLOR_XRGB,
                framebuffer_ptr,
                framebuffer_pitch as u32,
                framebuffer_width as u32,
                framebuffer_height as u32,
            );
        }
    }
}
