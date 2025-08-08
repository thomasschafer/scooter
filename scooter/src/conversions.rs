use crossterm::event::{KeyCode as CrosstermKeyCode, KeyModifiers as CrosstermKeyModifiers};
use scooter_core::fields::{KeyCode, KeyModifiers};

pub fn convert_key_code(code: CrosstermKeyCode) -> Option<KeyCode> {
    match code {
        CrosstermKeyCode::Backspace => Some(KeyCode::Backspace),
        CrosstermKeyCode::Char(c) => Some(KeyCode::Char(c)),
        CrosstermKeyCode::Delete => Some(KeyCode::Delete),
        CrosstermKeyCode::End => Some(KeyCode::End),
        CrosstermKeyCode::Enter => Some(KeyCode::Enter),
        CrosstermKeyCode::Left => Some(KeyCode::Left),
        CrosstermKeyCode::Home => Some(KeyCode::Home),
        CrosstermKeyCode::Right => Some(KeyCode::Right),
        _ => None, // Other key codes not supported by scooter-core
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
