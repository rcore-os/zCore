use alloc::sync::Arc;

use sdl2::{event::Event, keyboard::Keycode, EventPump};
use sdl2::{pixels::PixelFormatEnum, render::Canvas, video::Window};

use crate::prelude::ColorFormat;
use crate::scheme::DisplayScheme;

pub struct SdlWindow {
    canvas: Canvas<Window>,
    event_pump: EventPump,
    display: Arc<dyn DisplayScheme>,
}

impl SdlWindow {
    pub fn new(title: &str, display: Arc<dyn DisplayScheme>) -> Self {
        let sdl_context = sdl2::init().unwrap();
        let video_subsystem = sdl_context.video().unwrap();
        let window = video_subsystem
            .window(title, display.info().width, display.info().height)
            .position_centered()
            .build()
            .unwrap();

        let event_pump = sdl_context.event_pump().unwrap();
        let canvas = window.into_canvas().build().unwrap();
        let mut ret = Self {
            display,
            canvas,
            event_pump,
        };
        ret.flush();
        ret
    }

    pub fn flush(&mut self) {
        let info = self.display.info();
        let texture_creator = self.canvas.texture_creator();
        let format: PixelFormatEnum = info.format.into();
        let mut texture = texture_creator
            .create_texture_streaming(format, info.width, info.height)
            .unwrap();

        let buf = unsafe { self.display.raw_fb() };
        texture.update(None, buf, info.pitch() as usize).unwrap();
        self.canvas.copy(&texture, None, None).unwrap();
        self.canvas.present();
    }

    pub fn is_quit(&mut self) -> bool {
        for event in self.event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => return true,
                _ => {}
            }
        }
        false
    }
}

impl core::convert::From<ColorFormat> for PixelFormatEnum {
    fn from(format: ColorFormat) -> Self {
        match format {
            ColorFormat::RGB332 => Self::RGB332,
            ColorFormat::RGB565 => Self::RGB565,
            ColorFormat::RGB888 => Self::BGR24, // notice: BGR24 means R at the highest address, B at the lowest address.
            ColorFormat::ARGB8888 => Self::ARGB8888,
        }
    }
}
