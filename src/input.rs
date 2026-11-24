use egui::{Key, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SplitStacked,
    SplitSideBySide,
    ClosePane,
    NewTab,
    NewWindow,
    NextTab,
    PrevTab,
    NextPane,
    PrevPane,
    Focus(Dir),
    Resize(Dir),
    ToggleZoom,
    FontBigger,
    FontSmaller,
    FontReset,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    BroadcastAll,
    BroadcastOff,
    ToggleSidebar,
    /// Reopen the newest History entry.
    ReopenClosed,
    ShowHistory,
    OpenPalette,
    OpenSettings,
    Quit,
}

fn arrow(key: Key) -> Option<Dir> {
    Some(match key {
        Key::ArrowLeft => Dir::Left,
        Key::ArrowRight => Dir::Right,
        Key::ArrowUp => Dir::Up,
        Key::ArrowDown => Dir::Down,
        _ => return None,
    })
}

/// Terminator-style keybindings. Copy/paste (Ctrl+Shift+C/V) arrive as egui Copy/Paste events.
pub fn action_for(key: Key, m: Modifiers) -> Option<Action> {
    use Action::*;
    let (ctrl, shift, alt) = (m.ctrl, m.shift, m.alt);

    if ctrl && shift && !alt {
        if let Some(d) = arrow(key) {
            return Some(Resize(d));
        }
        return match key {
            Key::O => Some(SplitStacked),
            Key::E => Some(SplitSideBySide),
            Key::W => Some(ClosePane),
            Key::T => Some(NewTab),
            Key::Q => Some(Quit),
            Key::X => Some(ToggleZoom),
            Key::B => Some(ToggleSidebar),
            Key::R => Some(ReopenClosed),
            Key::H => Some(ShowHistory),
            Key::N => Some(NewWindow),
            Key::P => Some(OpenPalette),
            Key::Tab => Some(PrevTab),
            Key::Plus | Key::Equals => Some(FontBigger),
            _ => None,
        };
    }
    if ctrl && !shift && !alt {
        return match key {
            Key::PageUp => Some(PrevTab),
            Key::PageDown | Key::Tab => Some(NextTab),
            Key::Plus | Key::Equals => Some(FontBigger),
            Key::Minus => Some(FontSmaller),
            Key::Num0 => Some(FontReset),
            Key::Comma => Some(OpenSettings),
            _ => None,
        };
    }
    if alt && !ctrl && !shift {
        if let Some(d) = arrow(key) {
            return Some(Focus(d));
        }
        return match key {
            Key::A => Some(BroadcastAll),
            Key::O => Some(BroadcastOff),
            _ => None,
        };
    }
    if shift && !ctrl && !alt {
        return match key {
            Key::PageUp => Some(ScrollPageUp),
            Key::PageDown => Some(ScrollPageDown),
            Key::Home => Some(ScrollTop),
            Key::End => Some(ScrollBottom),
            _ => None,
        };
    }
    None
}

fn single_char(key: Key) -> Option<u8> {
    let name = key.name();
    (name.len() == 1).then(|| name.as_bytes()[0])
}

/// Bytes to send to the PTY for a key press that does not produce a Text event on its own
/// (special keys, Ctrl/Alt combos). Returns `(bytes, suppress_following_text)`.
pub fn key_to_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<(Vec<u8>, bool)> {
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let modp = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8;
    let cursor = |c: char| {
        if modp > 1 {
            format!("\x1b[1;{modp}{c}")
        } else if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
    };
    let ss3 = |c: char| if modp > 1 { format!("\x1b[1;{modp}{c}") } else { format!("\x1bO{c}") };
    let tilde = |n: u8| if modp > 1 { format!("\x1b[{n};{modp}~") } else { format!("\x1b[{n}~") };

    let s: String = match key {
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => ss3('P'),
        Key::F2 => ss3('Q'),
        Key::F3 => ss3('R'),
        Key::F4 => ss3('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        // Shift+Enter / Alt+Enter -> ESC CR: a newline in Claude Code & friends instead of submit.
        Key::Enter if m.shift || m.alt => "\x1b\r".into(),
        Key::Enter => "\r".into(),
        Key::Tab if m.shift => "\x1b[Z".into(),
        Key::Tab => "\t".into(),
        Key::Backspace if m.ctrl => "\x08".into(),
        Key::Backspace if m.alt => "\x1b\x7f".into(),
        Key::Backspace => "\x7f".into(),
        Key::Escape => "\x1b".into(),
        _ => {
            let ch = single_char(key);
            if m.ctrl {
                let b = match (key, ch) {
                    (Key::Space, _) => 0x00,
                    (_, Some(c)) if c.is_ascii_alphabetic() => c.to_ascii_lowercase() & 0x1f,
                    (_, Some(b'2')) => 0x00,
                    (_, Some(c @ b'3'..=b'7')) => c - b'3' + 0x1b,
                    (_, Some(b'8')) => 0x7f,
                    (Key::OpenBracket, _) => 0x1b,
                    (Key::Backslash, _) => 0x1c,
                    (Key::CloseBracket, _) => 0x1d,
                    (Key::Slash, _) => 0x1f,
                    _ => return None,
                };
                let mut v = Vec::new();
                if m.alt {
                    v.push(0x1b);
                }
                v.push(b);
                return Some((v, true));
            }
            if m.alt {
                // Meta sends ESC prefix: Alt+b -> ESC b (readline word-back etc.)
                if let Some(c) = ch.filter(|c| c.is_ascii_alphanumeric()) {
                    let c = if m.shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() };
                    return Some((vec![0x1b, c], true));
                }
            }
            return None;
        }
    };
    Some((s.into_bytes(), false))
}

/// Encode a mouse button event at a 0-based cell (xterm protocol). Wheel up = 64, down = 65.
pub fn mouse_report(button: u8, col: usize, row: usize, sgr: bool) -> Vec<u8> {
    if sgr {
        format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes()
    } else {
        // Legacy X10 encoding: each value offset by 32, limited to 223.
        let enc = |v: usize| (32 + v + 1).min(255) as u8;
        vec![0x1b, b'[', b'M', 32 + button, enc(col), enc(row)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_reports() {
        assert_eq!(mouse_report(64, 0, 0, true), b"\x1b[<64;1;1M");
        assert_eq!(mouse_report(65, 9, 4, false), vec![0x1b, b'[', b'M', 97, 42, 37]);
    }

    #[test]
    fn ctrl_c_is_etx() {
        let m = Modifiers { ctrl: true, command: true, ..Default::default() };
        assert_eq!(key_to_bytes(Key::C, m, false).unwrap().0, vec![3]);
    }

    #[test]
    fn plain_letter_is_left_to_text_event() {
        assert!(key_to_bytes(Key::A, Modifiers::NONE, false).is_none());
    }

    #[test]
    fn app_cursor_arrows() {
        assert_eq!(key_to_bytes(Key::ArrowUp, Modifiers::NONE, true).unwrap().0, b"\x1bOA");
        assert_eq!(key_to_bytes(Key::ArrowUp, Modifiers::NONE, false).unwrap().0, b"\x1b[A");
    }

    #[test]
    fn terminator_bindings() {
        let cs = Modifiers { ctrl: true, shift: true, command: true, ..Default::default() };
        assert_eq!(action_for(Key::O, cs), Some(Action::SplitStacked));
        assert_eq!(action_for(Key::E, cs), Some(Action::SplitSideBySide));
        let alt = Modifiers { alt: true, ..Default::default() };
        assert_eq!(action_for(Key::ArrowLeft, alt), Some(Action::Focus(Dir::Left)));
    }
}
