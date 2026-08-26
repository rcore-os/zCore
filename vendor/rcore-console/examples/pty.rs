use std::io::{stdin, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::{cell::RefCell, convert::Infallible, fs::File, process::Command, time::Duration};

use embedded_graphics_simulator::{
    OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use libc::{self, winsize};
use mio::{unix::EventedFd, Events, Poll, PollOpt, Ready, Token};
use pty::fork::Fork;
use termios::{cfmakeraw, tcsetattr, Termios, TCSANOW};

use rcore_console::{Console, DrawTarget, OriginDimensions, Pixel, Rgb888, Size};

const DISPLAY_SIZE: Size = Size::new(800, 600);

fn main() {
    let fork = Fork::from_ptmx().unwrap();

    if let Ok(mut master) = fork.is_parent() {
        env_logger::init();
        let display = SimulatorDisplay::<Rgb888>::new(DISPLAY_SIZE);
        let display = RefCell::new(display);

        let mut console = Console::on_frame_buffer(DisplayWrapper(&display));
        let poll = Poll::new().unwrap();
        poll.register(
            &EventedFd(&master.as_raw_fd()),
            Token(0),
            Ready::readable(),
            PollOpt::edge(),
        )
        .unwrap();

        // set to raw mode
        let fd = stdin().as_raw_fd();
        let mut termios = Termios::from_fd(fd).unwrap();
        cfmakeraw(&mut termios);
        tcsetattr(fd, TCSANOW, &termios).unwrap();

        let ws = winsize {
            ws_row: console.rows() as libc::c_ushort,
            ws_col: console.columns() as libc::c_ushort,
            ws_xpixel: DISPLAY_SIZE.width as libc::c_ushort,
            ws_ypixel: DISPLAY_SIZE.height as libc::c_ushort,
        };
        let res = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws as *const _) };
        if res < 0 {
            panic!(
                "ioctl TIOCSWINSZ failed: {}",
                std::io::Error::last_os_error()
            );
        }

        let mut stdin = unsafe { File::from_raw_fd(fd) };
        poll.register(
            &EventedFd(&fd),
            Token(1),
            Ready::readable(),
            PollOpt::edge(),
        )
        .unwrap();
        let mut events = Events::with_capacity(1024);

        let output_settings = OutputSettingsBuilder::new().build();
        let mut window = Window::new("Example", &output_settings);

        loop {
            poll.poll(&mut events, Some(Duration::from_millis(10)))
                .unwrap();

            let mut buffer = [0u8; 10240];
            for event in events.iter() {
                match event.token() {
                    Token(0) => {
                        let len = master.read(&mut buffer).unwrap();
                        for c in &buffer[..len] {
                            console.write_byte(*c);
                        }
                    }
                    Token(1) => {
                        let len = stdin.read(&mut buffer).unwrap();
                        master.write_all(&buffer[..len]).unwrap();
                    }
                    _ => unreachable!(),
                }
            }

            while let Some(byte) = console.pop_report() {
                master.write_all(&[byte]).unwrap();
            }

            window.update(&display.borrow_mut());
            if window.events().any(|e| e == SimulatorEvent::Quit) {
                break;
            }
        }
    } else {
        let mut args = std::env::args_os();
        args.next(); // skip myself
        let name = args.next().unwrap();
        Command::new(name)
            .args(args)
            .status()
            .expect("could not execute program");
    }
}

struct DisplayWrapper<'a>(&'a RefCell<SimulatorDisplay<Rgb888>>);

impl DrawTarget for DisplayWrapper<'_> {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        self.0.borrow_mut().draw_iter(pixels)
    }
}

impl OriginDimensions for DisplayWrapper<'_> {
    fn size(&self) -> Size {
        self.0.borrow().size()
    }
}
