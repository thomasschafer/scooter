use crossterm::event::{
    KeyCode as CrosstermKeyCode, KeyEvent, KeyModifiers as CrosstermKeyModifiers,
};
use scooter_core::fields::{KeyCode, KeyModifiers};

pub fn convert_key_code(code: CrosstermKeyCode) -> Option<KeyCode> {
    match code {
        CrosstermKeyCode::BackTab => Some(KeyCode::BackTab),
        CrosstermKeyCode::Backspace => Some(KeyCode::Backspace),
        CrosstermKeyCode::Char(char) => Some(KeyCode::Char(char)),
        CrosstermKeyCode::Delete => Some(KeyCode::Delete),
        CrosstermKeyCode::Down => Some(KeyCode::Down),
        CrosstermKeyCode::End => Some(KeyCode::End),
        CrosstermKeyCode::Enter => Some(KeyCode::Enter),
        CrosstermKeyCode::Esc => Some(KeyCode::Esc),
        CrosstermKeyCode::Home => Some(KeyCode::Home),
        CrosstermKeyCode::Left => Some(KeyCode::Left),
        CrosstermKeyCode::PageDown => Some(KeyCode::PageDown),
        CrosstermKeyCode::PageUp => Some(KeyCode::PageUp),
        CrosstermKeyCode::Right => Some(KeyCode::Right),
        CrosstermKeyCode::Tab => Some(KeyCode::Tab),
        CrosstermKeyCode::Up => Some(KeyCode::Up),
        _ => None,
    }
}

pub fn convert_key_modifiers(modifiers: CrosstermKeyModifiers) -> KeyModifiers {
    macro_rules! add_modifiers {
        ($result:expr, $modifiers:expr, $($flag:ident),+) => {
            $(
                if $modifiers.contains(CrosstermKeyModifiers::$flag) {
                    $result |= KeyModifiers::$flag;
                }
            )+
        };
    }

    let mut result = KeyModifiers::NONE;
    add_modifiers!(result, modifiers, SHIFT, CONTROL, ALT, SUPER, HYPER, META);
    result
}

pub fn convert_key_event(key: &KeyEvent) -> Option<(KeyCode, KeyModifiers)> {
    convert_key_code(key.code).map(|code| (code, convert_key_modifiers(key.modifiers)))
}
