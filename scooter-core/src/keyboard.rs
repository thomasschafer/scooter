// This code is copied from Helix: https://github.com/helix-editor/helix/blob/d79cce4e/helix-view/src/keyboard.rs
use anyhow::anyhow;
use bitflags::bitflags;

bitflags! {
    /// Represents key modifiers (shift, control, alt).
    #[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Copy, Hash)]
    pub struct KeyModifiers: u8 {
        const SHIFT = 0b0000_0001;
        const CONTROL = 0b0000_0010;
        const ALT = 0b0000_0100;
        const SUPER = 0b0000_1000;
        const HYPER = 0b0001_0000;
        const META = 0b0010_0000;
        const NONE = 0b0000_0000;
    }
}

#[cfg(feature = "term")]
impl From<KeyModifiers> for crossterm::event::KeyModifiers {
    fn from(key_modifiers: KeyModifiers) -> Self {
        use crossterm::event::KeyModifiers as CKeyModifiers;

        let mut result = CKeyModifiers::NONE;

        if key_modifiers.contains(KeyModifiers::SHIFT) {
            result.insert(CKeyModifiers::SHIFT);
        }
        if key_modifiers.contains(KeyModifiers::CONTROL) {
            result.insert(CKeyModifiers::CONTROL);
        }
        if key_modifiers.contains(KeyModifiers::ALT) {
            result.insert(CKeyModifiers::ALT);
        }
        if key_modifiers.contains(KeyModifiers::SUPER) {
            result.insert(CKeyModifiers::SUPER);
        }

        result
    }
}

#[cfg(feature = "term")]
impl From<crossterm::event::KeyModifiers> for KeyModifiers {
    fn from(val: crossterm::event::KeyModifiers) -> Self {
        use crossterm::event::KeyModifiers as CKeyModifiers;

        let mut result = KeyModifiers::NONE;

        if val.contains(CKeyModifiers::SHIFT) {
            result.insert(KeyModifiers::SHIFT);
        }
        if val.contains(CKeyModifiers::CONTROL) {
            result.insert(KeyModifiers::CONTROL);
        }
        if val.contains(CKeyModifiers::ALT) {
            result.insert(KeyModifiers::ALT);
        }
        if val.contains(CKeyModifiers::SUPER) {
            result.insert(KeyModifiers::SUPER);
        }

        result
    }
}

/// Represents a media key (as part of [`KeyCode::Media`]).
#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Copy, Hash)]
pub enum MediaKeyCode {
    /// Play media key.
    Play,
    /// Pause media key.
    Pause,
    /// Play/Pause media key.
    PlayPause,
    /// Reverse media key.
    Reverse,
    /// Stop media key.
    Stop,
    /// Fast-forward media key.
    FastForward,
    /// Rewind media key.
    Rewind,
    /// Next-track media key.
    TrackNext,
    /// Previous-track media key.
    TrackPrevious,
    /// Record media key.
    Record,
    /// Lower-volume media key.
    LowerVolume,
    /// Raise-volume media key.
    RaiseVolume,
    /// Mute media key.
    MuteVolume,
}

#[cfg(feature = "term")]
impl From<MediaKeyCode> for crossterm::event::MediaKeyCode {
    fn from(media_key_code: MediaKeyCode) -> Self {
        use crossterm::event::MediaKeyCode as CMediaKeyCode;

        match media_key_code {
            MediaKeyCode::Play => CMediaKeyCode::Play,
            MediaKeyCode::Pause => CMediaKeyCode::Pause,
            MediaKeyCode::PlayPause => CMediaKeyCode::PlayPause,
            MediaKeyCode::Reverse => CMediaKeyCode::Reverse,
            MediaKeyCode::Stop => CMediaKeyCode::Stop,
            MediaKeyCode::FastForward => CMediaKeyCode::FastForward,
            MediaKeyCode::Rewind => CMediaKeyCode::Rewind,
            MediaKeyCode::TrackNext => CMediaKeyCode::TrackNext,
            MediaKeyCode::TrackPrevious => CMediaKeyCode::TrackPrevious,
            MediaKeyCode::Record => CMediaKeyCode::Record,
            MediaKeyCode::LowerVolume => CMediaKeyCode::LowerVolume,
            MediaKeyCode::RaiseVolume => CMediaKeyCode::RaiseVolume,
            MediaKeyCode::MuteVolume => CMediaKeyCode::MuteVolume,
        }
    }
}

#[cfg(feature = "term")]
impl From<crossterm::event::MediaKeyCode> for MediaKeyCode {
    fn from(val: crossterm::event::MediaKeyCode) -> Self {
        use crossterm::event::MediaKeyCode as CMediaKeyCode;

        match val {
            CMediaKeyCode::Play => MediaKeyCode::Play,
            CMediaKeyCode::Pause => MediaKeyCode::Pause,
            CMediaKeyCode::PlayPause => MediaKeyCode::PlayPause,
            CMediaKeyCode::Reverse => MediaKeyCode::Reverse,
            CMediaKeyCode::Stop => MediaKeyCode::Stop,
            CMediaKeyCode::FastForward => MediaKeyCode::FastForward,
            CMediaKeyCode::Rewind => MediaKeyCode::Rewind,
            CMediaKeyCode::TrackNext => MediaKeyCode::TrackNext,
            CMediaKeyCode::TrackPrevious => MediaKeyCode::TrackPrevious,
            CMediaKeyCode::Record => MediaKeyCode::Record,
            CMediaKeyCode::LowerVolume => MediaKeyCode::LowerVolume,
            CMediaKeyCode::RaiseVolume => MediaKeyCode::RaiseVolume,
            CMediaKeyCode::MuteVolume => MediaKeyCode::MuteVolume,
        }
    }
}

/// Represents a media key (as part of [`KeyCode::Modifier`]).
#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Copy, Hash)]
pub enum ModifierKeyCode {
    /// Left Shift key.
    LeftShift,
    /// Left Control key.
    LeftControl,
    /// Left Alt key.
    LeftAlt,
    /// Left Super key.
    LeftSuper,
    /// Left Hyper key.
    LeftHyper,
    /// Left Meta key.
    LeftMeta,
    /// Right Shift key.
    RightShift,
    /// Right Control key.
    RightControl,
    /// Right Alt key.
    RightAlt,
    /// Right Super key.
    RightSuper,
    /// Right Hyper key.
    RightHyper,
    /// Right Meta key.
    RightMeta,
    /// Iso Level3 Shift key.
    IsoLevel3Shift,
    /// Iso Level5 Shift key.
    IsoLevel5Shift,
}

#[cfg(feature = "term")]
impl From<ModifierKeyCode> for crossterm::event::ModifierKeyCode {
    fn from(modifier_key_code: ModifierKeyCode) -> Self {
        use crossterm::event::ModifierKeyCode as CModifierKeyCode;

        match modifier_key_code {
            ModifierKeyCode::LeftShift => CModifierKeyCode::LeftShift,
            ModifierKeyCode::LeftControl => CModifierKeyCode::LeftControl,
            ModifierKeyCode::LeftAlt => CModifierKeyCode::LeftAlt,
            ModifierKeyCode::LeftSuper => CModifierKeyCode::LeftSuper,
            ModifierKeyCode::LeftHyper => CModifierKeyCode::LeftHyper,
            ModifierKeyCode::LeftMeta => CModifierKeyCode::LeftMeta,
            ModifierKeyCode::RightShift => CModifierKeyCode::RightShift,
            ModifierKeyCode::RightControl => CModifierKeyCode::RightControl,
            ModifierKeyCode::RightAlt => CModifierKeyCode::RightAlt,
            ModifierKeyCode::RightSuper => CModifierKeyCode::RightSuper,
            ModifierKeyCode::RightHyper => CModifierKeyCode::RightHyper,
            ModifierKeyCode::RightMeta => CModifierKeyCode::RightMeta,
            ModifierKeyCode::IsoLevel3Shift => CModifierKeyCode::IsoLevel3Shift,
            ModifierKeyCode::IsoLevel5Shift => CModifierKeyCode::IsoLevel5Shift,
        }
    }
}

#[cfg(feature = "term")]
impl From<crossterm::event::ModifierKeyCode> for ModifierKeyCode {
    fn from(val: crossterm::event::ModifierKeyCode) -> Self {
        use crossterm::event::ModifierKeyCode as CModifierKeyCode;

        match val {
            CModifierKeyCode::LeftShift => ModifierKeyCode::LeftShift,
            CModifierKeyCode::LeftControl => ModifierKeyCode::LeftControl,
            CModifierKeyCode::LeftAlt => ModifierKeyCode::LeftAlt,
            CModifierKeyCode::LeftSuper => ModifierKeyCode::LeftSuper,
            CModifierKeyCode::LeftHyper => ModifierKeyCode::LeftHyper,
            CModifierKeyCode::LeftMeta => ModifierKeyCode::LeftMeta,
            CModifierKeyCode::RightShift => ModifierKeyCode::RightShift,
            CModifierKeyCode::RightControl => ModifierKeyCode::RightControl,
            CModifierKeyCode::RightAlt => ModifierKeyCode::RightAlt,
            CModifierKeyCode::RightSuper => ModifierKeyCode::RightSuper,
            CModifierKeyCode::RightHyper => ModifierKeyCode::RightHyper,
            CModifierKeyCode::RightMeta => ModifierKeyCode::RightMeta,
            CModifierKeyCode::IsoLevel3Shift => ModifierKeyCode::IsoLevel3Shift,
            CModifierKeyCode::IsoLevel5Shift => ModifierKeyCode::IsoLevel5Shift,
        }
    }
}

/// Represents a key.
#[allow(clippy::doc_markdown)]
#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Copy, Hash)]
pub enum KeyCode {
    /// Backspace key.
    Backspace,
    /// Enter key.
    Enter,
    /// Left arrow key.
    Left,
    /// Right arrow key.
    Right,
    /// Up arrow key.
    Up,
    /// Down arrow key.
    Down,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page up key.
    PageUp,
    /// Page down key.
    PageDown,
    /// Tab key.
    Tab,
    // Backtab key.
    BackTab,
    /// Delete key.
    Delete,
    /// Insert key.
    Insert,
    /// F key.
    ///
    /// `KeyCode::F(1)` represents F1 key, etc.
    F(u8),
    /// A character.
    ///
    /// `KeyCode::Char('c')` represents `c` character, etc.
    Char(char),
    /// Null.
    Null,
    /// Escape key.
    Esc,
    /// CapsLock key.
    CapsLock,
    /// ScrollLock key.
    ScrollLock,
    /// NumLock key.
    NumLock,
    /// PrintScreen key.
    PrintScreen,
    /// Pause key.
    Pause,
    /// Menu key.
    Menu,
    /// KeypadBegin key.
    KeypadBegin,
    /// A media key.
    Media(MediaKeyCode),
    /// A modifier key.
    Modifier(ModifierKeyCode),
}

#[cfg(feature = "term")]
impl From<KeyCode> for crossterm::event::KeyCode {
    fn from(key_code: KeyCode) -> Self {
        use crossterm::event::KeyCode as CKeyCode;

        match key_code {
            KeyCode::Backspace => CKeyCode::Backspace,
            KeyCode::Enter => CKeyCode::Enter,
            KeyCode::Left => CKeyCode::Left,
            KeyCode::Right => CKeyCode::Right,
            KeyCode::Up => CKeyCode::Up,
            KeyCode::Down => CKeyCode::Down,
            KeyCode::Home => CKeyCode::Home,
            KeyCode::End => CKeyCode::End,
            KeyCode::PageUp => CKeyCode::PageUp,
            KeyCode::PageDown => CKeyCode::PageDown,
            KeyCode::Tab => CKeyCode::Tab,
            KeyCode::BackTab => CKeyCode::BackTab,
            KeyCode::Delete => CKeyCode::Delete,
            KeyCode::Insert => CKeyCode::Insert,
            KeyCode::F(f_number) => CKeyCode::F(f_number),
            KeyCode::Char(character) => CKeyCode::Char(character),
            KeyCode::Null => CKeyCode::Null,
            KeyCode::Esc => CKeyCode::Esc,
            KeyCode::CapsLock => CKeyCode::CapsLock,
            KeyCode::ScrollLock => CKeyCode::ScrollLock,
            KeyCode::NumLock => CKeyCode::NumLock,
            KeyCode::PrintScreen => CKeyCode::PrintScreen,
            KeyCode::Pause => CKeyCode::Pause,
            KeyCode::Menu => CKeyCode::Menu,
            KeyCode::KeypadBegin => CKeyCode::KeypadBegin,
            KeyCode::Media(media_key_code) => CKeyCode::Media(media_key_code.into()),
            KeyCode::Modifier(modifier_key_code) => CKeyCode::Modifier(modifier_key_code.into()),
        }
    }
}

#[cfg(feature = "term")]
impl From<crossterm::event::KeyCode> for KeyCode {
    fn from(val: crossterm::event::KeyCode) -> Self {
        use crossterm::event::KeyCode as CKeyCode;

        match val {
            CKeyCode::Backspace => KeyCode::Backspace,
            CKeyCode::Enter => KeyCode::Enter,
            CKeyCode::Left => KeyCode::Left,
            CKeyCode::Right => KeyCode::Right,
            CKeyCode::Up => KeyCode::Up,
            CKeyCode::Down => KeyCode::Down,
            CKeyCode::Home => KeyCode::Home,
            CKeyCode::End => KeyCode::End,
            CKeyCode::PageUp => KeyCode::PageUp,
            CKeyCode::PageDown => KeyCode::PageDown,
            CKeyCode::Tab => KeyCode::Tab,
            CKeyCode::BackTab => unreachable!("BackTab should have been handled on KeyEvent level"),
            CKeyCode::Delete => KeyCode::Delete,
            CKeyCode::Insert => KeyCode::Insert,
            CKeyCode::F(f_number) => KeyCode::F(f_number),
            CKeyCode::Char(character) => KeyCode::Char(character),
            CKeyCode::Null => KeyCode::Null,
            CKeyCode::Esc => KeyCode::Esc,
            CKeyCode::CapsLock => KeyCode::CapsLock,
            CKeyCode::ScrollLock => KeyCode::ScrollLock,
            CKeyCode::NumLock => KeyCode::NumLock,
            CKeyCode::PrintScreen => KeyCode::PrintScreen,
            CKeyCode::Pause => KeyCode::Pause,
            CKeyCode::Menu => KeyCode::Menu,
            CKeyCode::KeypadBegin => KeyCode::KeypadBegin,
            CKeyCode::Media(media_key_code) => KeyCode::Media(media_key_code.into()),
            CKeyCode::Modifier(modifier_key_code) => KeyCode::Modifier(modifier_key_code.into()),
        }
    }
}

// This code is copied from Helix: https://github.com/helix-editor/helix/blob/d79cce4e/helix-view/src/input.rs

pub(crate) mod keys {
    pub(crate) const BACKSPACE: &str = "backspace";
    pub(crate) const ENTER: &str = "ret";
    pub(crate) const LEFT: &str = "left";
    pub(crate) const RIGHT: &str = "right";
    pub(crate) const UP: &str = "up";
    pub(crate) const DOWN: &str = "down";
    pub(crate) const HOME: &str = "home";
    pub(crate) const END: &str = "end";
    pub(crate) const PAGEUP: &str = "pageup";
    pub(crate) const PAGEDOWN: &str = "pagedown";
    pub(crate) const TAB: &str = "tab";
    pub(crate) const DELETE: &str = "del";
    pub(crate) const INSERT: &str = "ins";
    pub(crate) const NULL: &str = "null";
    pub(crate) const ESC: &str = "esc";
    pub(crate) const SPACE: &str = "space";
    pub(crate) const MINUS: &str = "minus";
    pub(crate) const LESS_THAN: &str = "lt";
    pub(crate) const GREATER_THAN: &str = "gt";
    pub(crate) const CAPS_LOCK: &str = "capslock";
    pub(crate) const SCROLL_LOCK: &str = "scrolllock";
    pub(crate) const NUM_LOCK: &str = "numlock";
    pub(crate) const PRINT_SCREEN: &str = "printscreen";
    pub(crate) const PAUSE: &str = "pause";
    pub(crate) const MENU: &str = "menu";
    pub(crate) const KEYPAD_BEGIN: &str = "keypadbegin";
    pub(crate) const PLAY: &str = "play";
    pub(crate) const PAUSE_MEDIA: &str = "pausemedia";
    pub(crate) const PLAY_PAUSE: &str = "playpause";
    pub(crate) const REVERSE: &str = "reverse";
    pub(crate) const STOP: &str = "stop";
    pub(crate) const FAST_FORWARD: &str = "fastforward";
    pub(crate) const REWIND: &str = "rewind";
    pub(crate) const TRACK_NEXT: &str = "tracknext";
    pub(crate) const TRACK_PREVIOUS: &str = "trackprevious";
    pub(crate) const RECORD: &str = "record";
    pub(crate) const LOWER_VOLUME: &str = "lowervolume";
    pub(crate) const RAISE_VOLUME: &str = "raisevolume";
    pub(crate) const MUTE_VOLUME: &str = "mutevolume";
    pub(crate) const LEFT_SHIFT: &str = "leftshift";
    pub(crate) const LEFT_CONTROL: &str = "leftcontrol";
    pub(crate) const LEFT_ALT: &str = "leftalt";
    pub(crate) const LEFT_SUPER: &str = "leftsuper";
    pub(crate) const LEFT_HYPER: &str = "lefthyper";
    pub(crate) const LEFT_META: &str = "leftmeta";
    pub(crate) const RIGHT_SHIFT: &str = "rightshift";
    pub(crate) const RIGHT_CONTROL: &str = "rightcontrol";
    pub(crate) const RIGHT_ALT: &str = "rightalt";
    pub(crate) const RIGHT_SUPER: &str = "rightsuper";
    pub(crate) const RIGHT_HYPER: &str = "righthyper";
    pub(crate) const RIGHT_META: &str = "rightmeta";
    pub(crate) const ISO_LEVEL_3_SHIFT: &str = "isolevel3shift";
    pub(crate) const ISO_LEVEL_5_SHIFT: &str = "isolevel5shift";
}

/// Represents a key event.
// We use a newtype here because we want to customize Deserialize and Display.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    // TODO: crossterm now supports kind & state if terminal supports kitty's extended protocol
}

impl KeyEvent {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }
}

impl std::str::FromStr for KeyEvent {
    type Err = anyhow::Error;

    #[allow(clippy::too_many_lines)]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut tokens: Vec<_> = s.split('-').collect();
        let mut code = match tokens.pop().ok_or_else(|| anyhow!("Missing key code"))? {
            keys::BACKSPACE => KeyCode::Backspace,
            keys::ENTER => KeyCode::Enter,
            keys::LEFT => KeyCode::Left,
            keys::RIGHT => KeyCode::Right,
            keys::UP => KeyCode::Up,
            keys::DOWN => KeyCode::Down,
            keys::HOME => KeyCode::Home,
            keys::END => KeyCode::End,
            keys::PAGEUP => KeyCode::PageUp,
            keys::PAGEDOWN => KeyCode::PageDown,
            keys::TAB => KeyCode::Tab,
            keys::DELETE => KeyCode::Delete,
            keys::INSERT => KeyCode::Insert,
            keys::NULL => KeyCode::Null,
            keys::ESC => KeyCode::Esc,
            keys::SPACE => KeyCode::Char(' '),
            keys::MINUS => KeyCode::Char('-'),
            keys::LESS_THAN => KeyCode::Char('<'),
            keys::GREATER_THAN => KeyCode::Char('>'),
            keys::CAPS_LOCK => KeyCode::CapsLock,
            keys::SCROLL_LOCK => KeyCode::ScrollLock,
            keys::NUM_LOCK => KeyCode::NumLock,
            keys::PRINT_SCREEN => KeyCode::PrintScreen,
            keys::PAUSE => KeyCode::Pause,
            keys::MENU => KeyCode::Menu,
            keys::KEYPAD_BEGIN => KeyCode::KeypadBegin,
            keys::PLAY => KeyCode::Media(MediaKeyCode::Play),
            keys::PAUSE_MEDIA => KeyCode::Media(MediaKeyCode::Pause),
            keys::PLAY_PAUSE => KeyCode::Media(MediaKeyCode::PlayPause),
            keys::STOP => KeyCode::Media(MediaKeyCode::Stop),
            keys::REVERSE => KeyCode::Media(MediaKeyCode::Reverse),
            keys::FAST_FORWARD => KeyCode::Media(MediaKeyCode::FastForward),
            keys::REWIND => KeyCode::Media(MediaKeyCode::Rewind),
            keys::TRACK_NEXT => KeyCode::Media(MediaKeyCode::TrackNext),
            keys::TRACK_PREVIOUS => KeyCode::Media(MediaKeyCode::TrackPrevious),
            keys::RECORD => KeyCode::Media(MediaKeyCode::Record),
            keys::LOWER_VOLUME => KeyCode::Media(MediaKeyCode::LowerVolume),
            keys::RAISE_VOLUME => KeyCode::Media(MediaKeyCode::RaiseVolume),
            keys::MUTE_VOLUME => KeyCode::Media(MediaKeyCode::MuteVolume),
            keys::LEFT_SHIFT => KeyCode::Modifier(ModifierKeyCode::LeftShift),
            keys::LEFT_CONTROL => KeyCode::Modifier(ModifierKeyCode::LeftControl),
            keys::LEFT_ALT => KeyCode::Modifier(ModifierKeyCode::LeftAlt),
            keys::LEFT_SUPER => KeyCode::Modifier(ModifierKeyCode::LeftSuper),
            keys::LEFT_HYPER => KeyCode::Modifier(ModifierKeyCode::LeftHyper),
            keys::LEFT_META => KeyCode::Modifier(ModifierKeyCode::LeftMeta),
            keys::RIGHT_SHIFT => KeyCode::Modifier(ModifierKeyCode::RightShift),
            keys::RIGHT_CONTROL => KeyCode::Modifier(ModifierKeyCode::RightControl),
            keys::RIGHT_ALT => KeyCode::Modifier(ModifierKeyCode::RightAlt),
            keys::RIGHT_SUPER => KeyCode::Modifier(ModifierKeyCode::RightSuper),
            keys::RIGHT_HYPER => KeyCode::Modifier(ModifierKeyCode::RightHyper),
            keys::RIGHT_META => KeyCode::Modifier(ModifierKeyCode::RightMeta),
            keys::ISO_LEVEL_3_SHIFT => KeyCode::Modifier(ModifierKeyCode::IsoLevel3Shift),
            keys::ISO_LEVEL_5_SHIFT => KeyCode::Modifier(ModifierKeyCode::IsoLevel5Shift),
            single if single.chars().count() == 1 => KeyCode::Char(single.chars().next().unwrap()),
            function if function.len() > 1 && function.starts_with('F') => {
                let function: String = function.chars().skip(1).collect();
                let function = str::parse::<u8>(&function)?;
                (function > 0 && function < 25)
                    .then_some(KeyCode::F(function))
                    .ok_or_else(|| anyhow!("Invalid function key '{function}'"))?
            }
            // Checking that the last token is empty ensures that this branch is only taken if
            // `-` is used as a code. For example this branch will not be taken for `S-` (which is
            // missing a code).
            _ if s.ends_with('-') && tokens.last().is_some_and(|t| t.is_empty()) => {
                if s == "-" {
                    return Ok(KeyEvent {
                        code: KeyCode::Char('-'),
                        modifiers: KeyModifiers::empty(),
                    });
                } else {
                    let suggestion = format!("{}-{}", s.trim_end_matches('-'), keys::MINUS);
                    return Err(anyhow!(
                        "Key '-' cannot be used with modifiers, use '{suggestion}' instead",
                    ));
                }
            }
            invalid => return Err(anyhow!("Invalid key code '{invalid}'")),
        };

        let mut modifiers = KeyModifiers::empty();
        for token in tokens {
            let flag = match token {
                "S" => KeyModifiers::SHIFT,
                "A" => KeyModifiers::ALT,
                "C" => KeyModifiers::CONTROL,
                "Meta" | "Cmd" | "Win" => KeyModifiers::SUPER,
                _ => return Err(anyhow!("Invalid key modifier '{token}-'")),
            };

            if modifiers.contains(flag) {
                return Err(anyhow!("Repeated key modifier '{token}-'"));
            }
            modifiers.insert(flag);
        }

        // Normalize character keys so that characters like C-S-r and C-R
        // are represented by equal KeyEvents.
        match code {
            KeyCode::Char(ch)
                if ch.is_ascii_lowercase() && modifiers.contains(KeyModifiers::SHIFT) =>
            {
                code = KeyCode::Char(ch.to_ascii_uppercase());
                modifiers.remove(KeyModifiers::SHIFT);
            }
            _ => (),
        }

        Ok(KeyEvent { code, modifiers })
    }
}
