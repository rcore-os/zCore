use alloc::sync::Arc;

use sdl2::{event::Event, EventPump};
use sdl2::{keyboard::Scancode, mouse::MouseButton};
use sdl2::{pixels::PixelFormatEnum, render::Canvas, video::Window};

use crate::input::input_event_codes::{key::*, rel::*, syn::*};
use crate::prelude::{ColorFormat, InputEvent, InputEventType};
use crate::scheme::{DisplayScheme, InputScheme};

pub struct SdlWindow {
    canvas: Canvas<Window>,
    event_pump: EventPump,
    display: Arc<dyn DisplayScheme>,
    handler: EventHandler,
    is_quit: bool,
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
            canvas,
            event_pump,
            display,
            handler: EventHandler::default(),
            is_quit: false,
        };
        ret.flush();
        ret
    }

    pub fn is_quit(&self) -> bool {
        self.is_quit
    }

    pub fn register_mouse(&mut self, mouse: Arc<dyn InputScheme>) {
        self.handler.mouse = Some(mouse);
    }

    pub fn register_keyboard(&mut self, keyboard: Arc<dyn InputScheme>) {
        self.handler.keyboard = Some(keyboard);
    }

    pub fn flush(&mut self) {
        let info = self.display.info();
        let texture_creator = self.canvas.texture_creator();
        let format: PixelFormatEnum = info.format.into();
        let mut texture = texture_creator
            .create_texture_streaming(format, info.width, info.height)
            .unwrap();

        texture
            .update(None, &self.display.fb(), info.pitch() as usize)
            .unwrap();
        self.canvas.copy(&texture, None, None).unwrap();
        self.canvas.present();
    }

    pub fn handle_events(&mut self) {
        for event in self.event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => self.is_quit = true,
                Event::MouseMotion { xrel, yrel, .. } => self.handler.mouse_move(xrel, yrel),
                Event::MouseButtonDown { mouse_btn, .. } => {
                    self.handler.mouse_button(mouse_btn, true)
                }
                Event::MouseButtonUp { mouse_btn, .. } => {
                    self.handler.mouse_button(mouse_btn, false)
                }
                Event::KeyDown {
                    scancode: Some(code),
                    ..
                } => {
                    self.handler.key(code, true);
                    if code == Scancode::Escape {
                        self.is_quit = true;
                    }
                }
                Event::KeyUp {
                    scancode: Some(code),
                    ..
                } => self.handler.key(code, false),
                _ => {}
            }
        }
    }
}

#[derive(Default)]
struct EventHandler {
    mouse: Option<Arc<dyn InputScheme>>,
    keyboard: Option<Arc<dyn InputScheme>>,
}

impl EventHandler {
    fn mouse_move(&self, rel_x: i32, rel_y: i32) {
        if let Some(ref m) = self.mouse {
            m.trigger(InputEvent {
                event_type: InputEventType::RelAxis,
                code: REL_X,
                value: rel_x,
            });
            m.trigger(InputEvent {
                event_type: InputEventType::RelAxis,
                code: REL_Y,
                value: rel_y,
            });
            m.trigger(InputEvent {
                event_type: InputEventType::Syn,
                code: SYN_REPORT,
                value: 0,
            });
        }
    }

    fn mouse_button(&self, btn: MouseButton, down: bool) {
        if let Some(ref m) = self.mouse {
            let code = match btn {
                MouseButton::Left => BTN_LEFT,
                MouseButton::Right => BTN_RIGHT,
                MouseButton::Middle => BTN_MIDDLE,
                _ => return,
            };
            let value = if down { 1 } else { 0 };
            m.trigger(InputEvent {
                event_type: InputEventType::Key,
                code,
                value,
            });
            m.trigger(InputEvent {
                event_type: InputEventType::Syn,
                code: SYN_REPORT,
                value: 0,
            });
        }
    }

    fn key(&self, scancode: Scancode, down: bool) {
        if let Some(ref m) = self.keyboard {
            if let Some(code) = scancode_2_eventcode(scancode) {
                let value = if down { 1 } else { 0 };
                m.trigger(InputEvent {
                    event_type: InputEventType::Key,
                    code,
                    value,
                });
                m.trigger(InputEvent {
                    event_type: InputEventType::Syn,
                    code: SYN_REPORT,
                    value: 0,
                });
            }
        }
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

fn scancode_2_eventcode(code: Scancode) -> Option<u16> {
    use Scancode::*;
    Some(match code {
        A => KEY_A,
        B => KEY_B,
        C => KEY_C,
        D => KEY_D,
        E => KEY_E,
        F => KEY_F,
        G => KEY_G,
        H => KEY_H,
        I => KEY_I,
        J => KEY_J,
        K => KEY_K,
        L => KEY_L,
        M => KEY_M,
        N => KEY_N,
        O => KEY_O,
        P => KEY_P,
        Q => KEY_Q,
        R => KEY_R,
        S => KEY_S,
        T => KEY_T,
        U => KEY_U,
        V => KEY_V,
        W => KEY_W,
        X => KEY_X,
        Y => KEY_Y,
        Z => KEY_Z,
        Num1 => KEY_1,
        Num2 => KEY_2,
        Num3 => KEY_3,
        Num4 => KEY_4,
        Num5 => KEY_5,
        Num6 => KEY_6,
        Num7 => KEY_7,
        Num8 => KEY_8,
        Num9 => KEY_9,
        Num0 => KEY_0,
        Return => KEY_ENTER,
        Escape => KEY_ESC,
        Backspace => KEY_BACKSPACE,
        Tab => KEY_TAB,
        Space => KEY_SPACE,
        Minus => KEY_MINUS,
        Equals => KEY_EQUAL,
        LeftBracket => KEY_LEFTBRACE,
        RightBracket => KEY_RIGHTBRACE,
        Backslash => KEY_BACKSLASH,
        NonUsHash => return None,
        Semicolon => KEY_SEMICOLON,
        Apostrophe => KEY_APOSTROPHE,
        Grave => KEY_GRAVE,
        Comma => KEY_COMMA,
        Period => KEY_DOT,
        Slash => KEY_SLASH,
        CapsLock => KEY_CAPSLOCK,
        F1 => KEY_F1,
        F2 => KEY_F2,
        F3 => KEY_F3,
        F4 => KEY_F4,
        F5 => KEY_F5,
        F6 => KEY_F6,
        F7 => KEY_F7,
        F8 => KEY_F8,
        F9 => KEY_F9,
        F10 => KEY_F10,
        F11 => KEY_F11,
        F12 => KEY_F12,
        PrintScreen => return None,
        ScrollLock => KEY_SCROLLLOCK,
        Pause => KEY_PAUSE,
        Insert => KEY_INSERT,
        Home => KEY_HOME,
        PageUp => KEY_PAGEUP,
        Delete => KEY_DELETE,
        End => KEY_END,
        PageDown => KEY_PAGEDOWN,
        Right => KEY_RIGHT,
        Left => KEY_LEFT,
        Down => KEY_DOWN,
        Up => KEY_UP,
        NumLockClear => KEY_NUMLOCK,
        KpDivide => KEY_KPSLASH,
        KpMultiply => KEY_KPASTERISK,
        KpMinus => KEY_KPMINUS,
        KpPlus => KEY_KPPLUS,
        KpEnter => KEY_KPENTER,
        Kp1 => KEY_KP1,
        Kp2 => KEY_KP2,
        Kp3 => KEY_KP3,
        Kp4 => KEY_KP4,
        Kp5 => KEY_KP5,
        Kp6 => KEY_KP6,
        Kp7 => KEY_KP7,
        Kp8 => KEY_KP8,
        Kp9 => KEY_KP9,
        Kp0 => KEY_KP0,
        KpPeriod => KEY_KPDOT,
        NonUsBackslash => KEY_102ND,
        Application => return None,
        Power => KEY_POWER,
        KpEquals => KEY_KPEQUAL,
        F13 => KEY_F13,
        F14 => KEY_F14,
        F15 => KEY_F15,
        F16 => KEY_F16,
        F17 => KEY_F17,
        F18 => KEY_F18,
        F19 => KEY_F19,
        F20 => KEY_F20,
        F21 => KEY_F21,
        F22 => KEY_F22,
        F23 => KEY_F23,
        F24 => KEY_F24,
        Execute => return None,
        Help => KEY_HELP,
        Menu => KEY_MENU,
        Select => return None,
        Stop => KEY_STOP,
        Again => KEY_AGAIN,
        Undo => KEY_UNDO,
        Cut => KEY_CUT,
        Copy => KEY_COPY,
        Paste => KEY_PASTE,
        Find => KEY_FIND,
        Mute => KEY_MUTE,
        VolumeUp => KEY_VOLUMEUP,
        VolumeDown => KEY_VOLUMEDOWN,
        KpComma => KEY_KPCOMMA,
        KpEqualsAS400 => return None,
        International1 => KEY_RO,
        International2 => KEY_KATAKANAHIRAGANA,
        International3 => KEY_YEN,
        International4 => KEY_HENKAN,
        International5 => KEY_MUHENKAN,
        International6 => return None,
        International7 => return None,
        International8 => return None,
        International9 => return None,
        Lang1 => KEY_HANGEUL,
        Lang2 => KEY_HANJA,
        Lang3 => KEY_KATAKANA,
        Lang4 => KEY_HIRAGANA,
        Lang5 => return None,
        Lang6 => return None,
        Lang7 => return None,
        Lang8 => return None,
        Lang9 => return None,
        AltErase => KEY_ALTERASE,
        SysReq => KEY_SYSRQ,
        Cancel => KEY_CANCEL,
        Clear => return None,
        Prior => return None,
        Return2 => return None,
        Separator => return None,
        Out => return None,
        Oper => return None,
        ClearAgain => return None,
        CrSel => return None,
        ExSel => return None,
        Kp00 => return None,
        Kp000 => return None,
        ThousandsSeparator => return None,
        DecimalSeparator => return None,
        CurrencyUnit => return None,
        CurrencySubUnit => return None,
        KpLeftParen => KEY_KPLEFTPAREN,
        KpRightParen => KEY_KPRIGHTPAREN,
        KpLeftBrace => return None,
        KpRightBrace => return None,
        KpTab => return None,
        KpBackspace => return None,
        KpA => return None,
        KpB => return None,
        KpC => return None,
        KpD => return None,
        KpE => return None,
        KpF => return None,
        KpXor => return None,
        KpPower => return None,
        KpPercent => return None,
        KpLess => return None,
        KpGreater => return None,
        KpAmpersand => return None,
        KpDblAmpersand => return None,
        KpVerticalBar => return None,
        KpDblVerticalBar => return None,
        KpColon => return None,
        KpHash => return None,
        KpSpace => return None,
        KpAt => return None,
        KpExclam => return None,
        KpMemStore => return None,
        KpMemRecall => return None,
        KpMemClear => return None,
        KpMemAdd => return None,
        KpMemSubtract => return None,
        KpMemMultiply => return None,
        KpMemDivide => return None,
        KpPlusMinus => KEY_KPPLUSMINUS,
        KpClear => return None,
        KpClearEntry => return None,
        KpBinary => return None,
        KpOctal => return None,
        KpDecimal => return None,
        KpHexadecimal => return None,
        LCtrl => KEY_LEFTCTRL,
        LShift => KEY_LEFTSHIFT,
        LAlt => KEY_LEFTALT,
        LGui => KEY_LEFTMETA,
        RCtrl => KEY_RIGHTCTRL,
        RShift => KEY_RIGHTSHIFT,
        RAlt => KEY_RIGHTALT,
        RGui => KEY_RIGHTMETA,
        Mode => return None,
        AudioNext => KEY_NEXTSONG,
        AudioPrev => KEY_PREVIOUSSONG,
        AudioStop => return None,
        AudioPlay => KEY_PLAYPAUSE,
        AudioMute => return None,
        MediaSelect => return None,
        Www => return None,
        Mail => KEY_MAIL,
        Calculator => KEY_CALC,
        Computer => KEY_COMPUTER,
        AcSearch => KEY_SEARCH,
        AcHome => KEY_HOMEPAGE,
        AcBack => KEY_BACK,
        AcForward => KEY_FORWARD,
        AcStop => return None,
        AcRefresh => KEY_REFRESH,
        AcBookmarks => KEY_BOOKMARKS,
        BrightnessDown => KEY_BRIGHTNESSDOWN,
        BrightnessUp => KEY_BRIGHTNESSUP,
        DisplaySwitch => KEY_SWITCHVIDEOMODE,
        KbdIllumToggle => KEY_KBDILLUMTOGGLE,
        KbdIllumDown => KEY_KBDILLUMDOWN,
        KbdIllumUp => KEY_KBDILLUMUP,
        Eject => KEY_EJECTCD,
        Sleep => KEY_SLEEP,
        App1 => return None,
        App2 => return None,
        Num => return None,
    })
}
