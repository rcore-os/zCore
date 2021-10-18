#[cfg(feature = "graphic")]
pub fn run_graphic_service() {
    use crate::drivers::{display, input};
    use zcore_drivers::mock::graphic::sdl::SdlWindow;

    let mut window = SdlWindow::new("zcore-libos", display::first_unwrap());
    if let Some(i) = input::find("mock-mouse-input") {
        window.register_mouse(i);
    }
    if let Some(i) = input::find("mock-keyboard-input") {
        window.register_keyboard(i);
    }

    while !window.is_quit() {
        window.handle_events();
        window.flush();
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}
