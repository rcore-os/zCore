use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{convert::Infallible, fmt::Write, thread};

use embedded_graphics_simulator::{
    OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};

use rcore_console::{Console, DrawTarget, OriginDimensions, Pixel, Rgb888, Size};

const DISPLAY_SIZE: Size = Size::new(1280, 720);

fn main() {
    env_logger::init();
    let display = SimulatorDisplay::<Rgb888>::new(DISPLAY_SIZE);
    let display = Arc::new(Mutex::new(display));
    let mut console = Console::on_frame_buffer(DisplayWrapper(display.clone()));

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1000));

        let mut args = std::env::args_os();
        args.next(); // skip myself
        let fname = args
            .next()
            .expect("Usage: replay <ANSI_ESCAPE_SEQUENCE_FILE>");
        let input = std::fs::read_to_string(fname.clone()).unwrap();
        println!("Read {} bytes from {:?}", input.len(), fname);

        let time = Instant::now();
        console.write_str(&input).unwrap();
        println!("Render time: {:?}", time.elapsed());
    });

    let output_settings = OutputSettingsBuilder::new().build();
    let mut window = Window::new("Example", &output_settings);
    loop {
        window.update(&display.lock().unwrap());
        if window.events().any(|e| e == SimulatorEvent::Quit) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
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
