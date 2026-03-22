use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::Size;
use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use limine::framebuffer::Framebuffer;
use spin::{Mutex, MutexGuard};

pub struct FrameBufferTarget<'a> {
    framebuffer: MutexGuard<'a, Framebuffer<'static>>,
    pub width: u64,
    pub height: u64,
    pitch: u64,
}

impl<'a> FrameBufferTarget<'a> {
    pub fn new(framebuffer: MutexGuard<'a, Framebuffer<'static>>) -> Self {
        let width = framebuffer.width();
        let height = framebuffer.height();
        let pitch = framebuffer.pitch();
        Self {
            framebuffer,
            width,
            height,
            pitch,
        }
    }

    fn write_pixel(&mut self, x: u64, y: u64, color: Rgb888) {
        if x >= self.width || y >= self.height {
            return;
        }

        let px_offset = y * self.pitch + x * 4;
        let px_ptr = unsafe {
            self.framebuffer
                .addr()
                .add(px_offset as usize)
                .cast::<u32>()
        };

        let color: u32 =
            ((color.r() as u32) << 16) | ((color.g() as u32) << 8) | (color.b() as u32);

        unsafe { px_ptr.write(color) };
    }

    pub fn width(&self) -> u64 {
        self.width
    }

    pub fn height(&self) -> u64 {
        self.height
    }
}

impl<'a> DrawTarget for FrameBufferTarget<'a> {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            self.write_pixel(coord.x as u64, coord.y as u64, color);
        }
        Ok(())
    }
}

impl<'a> OriginDimensions for FrameBufferTarget<'a> {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}
