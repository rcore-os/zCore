use sdl2::{event::Event, keyboard::Keycode, EventPump};
use sdl2::{pixels::PixelFormatEnum, render::Canvas, video::Window};

use crate::display::{ColorFormat, DisplayInfo};
use crate::scheme::DisplayScheme;

pub struct SdlWindow {
    canvas: Canvas<Window>,
    event_pump: EventPump,
    info: DisplayInfo,
}

impl SdlWindow {
    pub fn new(title: &str, info: DisplayInfo) -> Self {
        assert_eq!(info.format, ColorFormat::RGBA8888);
        let sdl_context = sdl2::init().unwrap();
        let video_subsystem = sdl_context.video().unwrap();
        let window = video_subsystem
            .window(title, info.width, info.height)
            .position_centered()
            .build()
            .unwrap();

        let event_pump = sdl_context.event_pump().unwrap();
        let mut canvas = window.into_canvas().build().unwrap();
        canvas.clear();
        canvas.present();
        Self {
            info,
            canvas,
            event_pump,
        }
    }

    pub fn flush(&mut self, display: &dyn DisplayScheme) {
        let texture_creator = self.canvas.texture_creator();
        let mut texture = texture_creator
            .create_texture_streaming(PixelFormatEnum::RGBA8888, self.info.width, self.info.height)
            .unwrap();

        let buf = unsafe { display.raw_fb() };
        texture
            .update(None, buf, display.info().width as usize * 4)
            .unwrap();
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
