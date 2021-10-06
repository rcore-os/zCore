use alloc::sync::Arc;
use core::convert::Infallible;
use core::ops::{Deref, DerefMut};

use rcore_console::{Console, ConsoleOnGraphic, DrawTarget, OriginDimensions, Pixel, Rgb888, Size};

use crate::scheme::DisplayScheme;

pub struct DisplayWrapper(Arc<dyn DisplayScheme>);

pub struct GraphicConsole {
    inner: ConsoleOnGraphic<DisplayWrapper>,
}

impl GraphicConsole {
    pub fn new(display: Arc<dyn DisplayScheme>) -> Self {
        Self {
            inner: Console::on_frame_buffer(DisplayWrapper(display)),
        }
    }
}

impl DrawTarget for DisplayWrapper {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for p in pixels {
            let color = unsafe { core::mem::transmute(p.1) };
            self.0.draw_pixel(p.0.x as u32, p.0.y as u32, color);
        }
        Ok(())
    }
}

impl OriginDimensions for DisplayWrapper {
    fn size(&self) -> Size {
        let info = self.0.info();
        Size::new(info.width, info.height)
    }
}

impl Deref for GraphicConsole {
    type Target = ConsoleOnGraphic<DisplayWrapper>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for GraphicConsole {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
