use crate::events::event_buffer::{
    EventBuffer, EventType, InputEvent, Keys, MouseButtons, MouseEvent,
};
use crate::graphics::FRAMEBUFFER_BYTES_PER_PIXEL;
use crate::graphics::color::{Rgba8888UNORM, rgba_to_xrgb};
use crate::graphics::framebuffer::FrameBufferTarget;
use crate::graphics::window::{INVALID_WINDOW_ID, Window, WindowBuffer, WindowID};
use crate::memory::{get_frame_allocator, usermem};
use crate::process::PID;
use crate::process::shared_state::{
    EVENT_BUFFER_ADDR, get_shared_program_data_buffer, get_shared_program_data_buffer_mut,
};
use crate::{serial_println, serial_println_core};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use embedded_graphics::pixelcolor::Rgb888;
use spin::{Mutex, MutexGuard, Once, RwLock};
use x86_64::structures::paging::PageTableFlags;
use x86_64::{PhysAddr, VirtAddr};

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
    INPUT_EVENT_BUFFER
        .get()
        .expect("input event buffer not initialized")
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
    currently_focused_window: AtomicU32,
    free_window_ids: Mutex<Vec<WindowID>>,
    // NOTE: cant sort directly by z_index cause the windows are keyed by their id == index
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
            currently_focused_window: AtomicU32::new(INVALID_WINDOW_ID),
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
        pid: PID,
    ) -> (WindowID, Arc<WindowBuffer>, Option<&'static EventBuffer>) {
        let buffer = Arc::new(WindowBuffer::new(width, height, x, y));
        let id = if let Some(free_id) = self.free_window_ids.lock().pop() {
            free_id
        } else {
            self.next_window_id.fetch_add(1, Ordering::Relaxed)
        };

        let (event_buffer, event_buffer_phys) = match self.allocate_event_buffer_for_window(id, pid)
        {
            Some((eb, eb_phys)) => (Some(eb), eb_phys),
            None => (None, PhysAddr::zero()),
        };
        let window = Window {
            id,
            x,
            y,
            z_index: 0,
            is_visible: true,
            buffer: buffer.clone(),
            event_buffer,
            event_buffer_phys,
        };

        {
            let mut windows = self.windows.write();
            if id as usize >= windows.len() {
                windows.push(window);
            } else {
                windows[id as usize] = window;
            }
        }

        self.focus_window(id);

        (id, buffer, event_buffer)
    }

    fn allocate_event_buffer_for_window(
        &self,
        window_id: WindowID,
        pid: PID,
    ) -> Option<(&'static EventBuffer, PhysAddr)> {
        serial_println_core!("allocate_event_buffer_for_window: window_id={}", window_id);

        let mut frame_allocator = get_frame_allocator();
        let (phys, kernel_vaddr) = match usermem::allocate_zeroed_page(&mut frame_allocator) {
            Some((phys, virt)) => (phys, virt),
            None => {
                serial_println_core!(
                    "allocate_event_buffer_for_window: Failed to allocate page for EventBuffer"
                );
                return None;
            }
        };

        unsafe {
            let buffer = &*(kernel_vaddr.as_u64() as *const EventBuffer);
            debug_assert_eq!(buffer.write_idx.load(Ordering::Relaxed), 0);
            debug_assert_eq!(buffer.read_idx.load(Ordering::Relaxed), 0);
            debug_assert_eq!(buffer.event_count.load(Ordering::Relaxed), 0);
        }

        serial_println_core!(
            "allocate_event_buffer_for_window: Allocated EventBuffer for window {}: phys={:?}, kernel_vaddr={:?}",
            window_id,
            phys,
            kernel_vaddr
        );

        {
            let user_memory_manager = &crate::memory::get_user_mem_mgr();
            serial_println_core!("allocate_event_buffer_for_window: locked get_user_mem_mgr");
            let mut pm = crate::process::process_manager::PROCESS_MANAGER.lock();
            serial_println_core!("allocate_event_buffer_for_window: locked PROCESS_MANAGER");

            serial_println_core!(
                "allocate_event_buffer_for_window: mapping EventBuffer for pid: {}",
                pid
            );
            if let Ok(proc) = pm.get_process_mut(pid) {
                serial_println_core!(
                    "allocate_event_buffer_for_window: found process for pid: {}",
                    pid
                );
                match user_memory_manager.map_specific_frame(
                    proc.memory_layout.top_page_table_phys,
                    //kernel_vaddr,
                    VirtAddr::new(EVENT_BUFFER_ADDR as u64),
                    phys,
                    PageTableFlags::PRESENT
                        | PageTableFlags::WRITABLE
                        | PageTableFlags::USER_ACCESSIBLE,
                    &mut frame_allocator,
                ) {
                    Ok(()) => {
                        serial_println_core!(
                            "allocate_event_buffer_for_window: EventBuffer mapped"
                        );
                    }
                    Err(e) => {
                        serial_println_core!(
                            "allocate_event_buffer_for_window: EventBuffer mapping error: {:?}",
                            e
                        );
                    }
                }
            }
        }
        serial_println_core!("allocate_event_buffer_for_window: end");

        Some((
            unsafe { &*(kernel_vaddr.as_u64() as *const EventBuffer) },
            phys,
        ))
    }

    pub fn set_z_index(&self, window_id: WindowID, z_index: u8) {
        if window_id != INVALID_WINDOW_ID {
            let mut windows = self.windows.write();
            let window = windows.get_mut(window_id as usize).unwrap();
            if window.z_index != z_index {
                window.z_index = z_index;
            }
        }
    }

    pub fn focus_window(&self, window_id: WindowID) {
        if window_id != INVALID_WINDOW_ID {
            let mut focused_window = self.currently_focused_window.load(Ordering::Acquire);
            if window_id != focused_window {
                let mut windows = self.windows.write();
                let max_z_index = windows.iter().max_by_key(|w| w.z_index).unwrap().z_index;

                match windows.get_mut(window_id as usize) {
                    Some(window) => {
                        window.z_index = max_z_index + 1;
                        self.currently_focused_window
                            .store(window_id, Ordering::Release);

                        if max_z_index > NORMALIZE_Z_INDEX_THRESHOLD {
                            let mut visible: Vec<&mut Window> =
                                windows.iter_mut().filter(|w| w.is_visible).collect();
                            visible.sort_by_key(|w| w.z_index);

                            for (i, window) in visible.iter_mut().enumerate() {
                                window.z_index = i as u8;
                            }
                        }

                        unsafe {
                            serial_println_core!(
                                "Compositor: setting focused_window_id to {}",
                                window_id
                            );
                            let mut shared_program_data = get_shared_program_data_buffer_mut();
                            shared_program_data
                                .focused_window_id
                                .store(window_id, Ordering::Release);
                        }
                    }
                    None => {
                        serial_println_core!(
                            "Compositor: focus_window - window does not exist: {}",
                            window_id
                        );
                    }
                }
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
                _ => {
                    if let Some(event_buffer) = self.get_focused_window_event_buffer() {
                        let _ = event_buffer.push(event);
                    }
                }
            }
        }
    }

    fn get_focused_window_event_buffer(&self) -> Option<&'static EventBuffer> {
        let focused_window_id = self.currently_focused_window.load(Ordering::Acquire);
        if focused_window_id == INVALID_WINDOW_ID {
            return None;
        }

        let windows = self.windows.read();
        windows
            .get(focused_window_id as usize)
            .and_then(|window| window.event_buffer)
    }

    pub fn handle_keyboard_event(&mut self, event: InputEvent) {
        let v = event.value;
        if v == Keys::LeftAlt as u32 {
            serial_println_core!("Compositor intercepted Left Alt")
        }
        if v == Keys::ArrowLeft as u32 {
            let focused_window_id = self.currently_focused_window.load(Ordering::Acquire);
            let new_focused_window_id = focused_window_id.saturating_sub(1);
            //TODO: focused_window_id.clamp(0, self.windows.len());
            serial_println_core!(
                "Switch focus from {} to {}",
                focused_window_id,
                new_focused_window_id
            );
            self.focus_window(new_focused_window_id);
        }
        if v == Keys::ArrowRight as u32 {
            let focused_window_id = self.currently_focused_window.load(Ordering::Acquire);
            let (new_focused_window_id, _) = focused_window_id.overflowing_add(1);
            //TODO: focused_window_id.clamp(0, self.windows.len());
            serial_println_core!(
                "Switch focus from {} to {}",
                focused_window_id,
                new_focused_window_id
            );
            self.focus_window(new_focused_window_id);
        }

        if let Some(event_buffer) = self.get_focused_window_event_buffer() {
            let _ = event_buffer.push(event);
        } else {
            serial_println_core!("No focused window event buffer to forward keyboard event");
        }
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

        if let Some(event_buffer) = self.get_focused_window_event_buffer() {
            let _ = event_buffer.push(event);
        } else {
            serial_println_core!("No focused window event buffer to forward mouse event");
        }
    }

    fn handle_mouse_click(&mut self, mouse_x: i32, mouse_y: i32, buttons: MouseButtons) {
        let focused_window_id = self.currently_focused_window.load(Ordering::Acquire);

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
        let target_window_id = {
            let windows = self.windows.read();

            windows
                .iter()
                .filter(|w| w.is_visible)
                .filter(|w| {
                    let x = w.x;
                    let y = w.y;
                    let width = w.buffer.width as i32;
                    let height = w.buffer.height as i32;

                    mouse_x >= x && mouse_x < x + width && mouse_y >= y && mouse_y < y + height
                })
                .max_by_key(|w| w.z_index)
                .map(|w| w.id)
        };

        if let Some(window_id) = target_window_id {
            if window_id != self.currently_focused_window.load(Ordering::Acquire) {
                self.focus_window(window_id);

                serial_println!("Focused window {} at ({}, {})", window_id, mouse_x, mouse_y);
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
