#[cfg(feature = "graphic")]
pub fn run_display_serve() {
    use zcore_drivers::mock::display::sdl::SdlWindow;

    let display = crate::drivers::display::first_unwrap();
    let mut window = SdlWindow::new("zcore-libos", display.info());
    while !window.is_quit() {
        window.flush(display.as_ref());
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
