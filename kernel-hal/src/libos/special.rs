#[cfg(feature = "graphic")]
pub fn run_graphic_service() {
    use crate::drivers::{all_display, all_input};
    use zcore_drivers::mock::graphic::sdl::SdlWindow;

    let mut window = SdlWindow::new("zcore-libos", all_display().first_unwrap());
    if let Some(i) = all_input().find("mock-mouse-input") {
        window.register_mouse(i);
    }
    if let Some(i) = all_input().find("mock-keyboard-input") {
        window.register_keyboard(i);
    }

    while !window.is_quit() {
        window.handle_events();
        window.flush();
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}
