use embedded_graphics_simulator::{
    OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use rcore_console::{Console, DrawTarget, OriginDimensions, Pixel, Rgb888, Size};

use std::convert::Infallible;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DISPLAY_SIZE: Size = Size::new(800, 600);

fn main() {
    env_logger::init();
    let display = SimulatorDisplay::<Rgb888>::new(DISPLAY_SIZE);
    let display = Arc::new(Mutex::new(display));

    let mut console = Console::on_frame_buffer(DisplayWrapper(display.clone()));
    std::thread::spawn(move || {
        for c in std::io::stdin().lock().bytes() {
            let c = c.unwrap();
            if c == 0xff {
                break;
            }
            console.write_byte(c);
        }
    });

    let output_settings = OutputSettingsBuilder::new().build();
    let mut window = Window::new("Example", &output_settings);
    loop {
        window.update(&display.lock().unwrap());
        if window.events().any(|e| e == SimulatorEvent::Quit) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct DisplayWrapper(Arc<Mutex<SimulatorDisplay<Rgb888>>>);

impl DrawTarget for DisplayWrapper {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        self.0.lock().unwrap().draw_iter(pixels)
    }
}

impl OriginDimensions for DisplayWrapper {
    fn size(&self) -> Size {
        self.0.lock().unwrap().size()
    }
}
