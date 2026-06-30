//! warpui Keystroke -> terminal PTY bytes. Ports Crane's
//! `view.rs::named_key_bytes` / `key_letter` and the ctrl/alt rules,
//! adapted to warpui's stringly-typed `Keystroke.key`.
//!
//! `Keystroke.key` is a lowercase string: a single char ("a", "7", " ")
//! or a special-key name ("up", "enter", "pageup", ...). There is no Key
//! enum, so we branch on the &str.

use warpui::keymap::Keystroke;

/// Encode a keystroke into the bytes to write to the PTY, or None if the
/// key should propagate to app-level handlers.
///
/// `app_cursor` is DECCKM (read from `term.is_app_cursor()`): when set,
/// arrows/home/end emit SS3 (`\x1bO…`) instead of CSI (`\x1b[…`).
pub fn keystroke_to_pty_bytes(ks: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    // 1) Ctrl + ascii letter -> C0 control code (letter - 'a' + 1).
    //    Plus the conventional ctrl-space / ctrl-2 -> NUL, ctrl-3 -> ESC.
    if ks.ctrl && !ks.alt && !ks.meta {
        if ks.key.chars().count() == 1 {
            let c = ks.key.chars().next().unwrap();
            if c.is_ascii_alphabetic() {
                return Some(vec![(c.to_ascii_lowercase() as u8) - b'a' + 1]);
            }
            match c {
                ' ' | '2' | '@' => return Some(vec![0x00]),
                '3' => return Some(vec![0x1b]),
                '4' | '\\' => return Some(vec![0x1c]),
                '5' | ']' => return Some(vec![0x1d]),
                '6' | '^' => return Some(vec![0x1e]),
                '7' | '/' | '_' => return Some(vec![0x1f]),
                _ => {}
            }
        }
    }

    // 2) Alt/Option: word-nav + ESC-prefixed input.
    if ks.alt && !ks.ctrl {
        match ks.key.as_str() {
            "left" => return Some(b"\x1bb".to_vec()),
            "right" => return Some(b"\x1bf".to_vec()),
            "backspace" => return Some(b"\x1b\x7f".to_vec()),
            k if k.chars().count() == 1 => {
                let mut v = vec![0x1b];
                v.extend_from_slice(k.as_bytes());
                return Some(v);
            }
            _ => {}
        }
    }

    // Shift+Tab -> backtab (CSI Z) for reverse-cycle (fzf, readline, focus-prev).
    if ks.key == "tab" && ks.shift && !ks.ctrl && !ks.alt && !ks.meta && !ks.cmd {
        return Some(b"\x1b[Z".to_vec());
    }

    // 3) Named keys (Crane named_key_bytes). app_cursor gates SS3 vs CSI.
    let named: Option<&[u8]> = match ks.key.as_str() {
        "enter" | "numpadenter" => Some(b"\r"),
        "tab" => Some(b"\t"),
        "backspace" => Some(&[0x7f]),
        // Function keys (xterm/DEC).
        "f1" => Some(b"\x1bOP"),
        "f2" => Some(b"\x1bOQ"),
        "f3" => Some(b"\x1bOR"),
        "f4" => Some(b"\x1bOS"),
        "f5" => Some(b"\x1b[15~"),
        "f6" => Some(b"\x1b[17~"),
        "f7" => Some(b"\x1b[18~"),
        "f8" => Some(b"\x1b[19~"),
        "f9" => Some(b"\x1b[20~"),
        "f10" => Some(b"\x1b[21~"),
        "f11" => Some(b"\x1b[23~"),
        "f12" => Some(b"\x1b[24~"),
        "escape" => Some(&[0x1b]),
        "up" => Some(if app_cursor { b"\x1bOA" } else { b"\x1b[A" }),
        "down" => Some(if app_cursor { b"\x1bOB" } else { b"\x1b[B" }),
        "right" => Some(if app_cursor { b"\x1bOC" } else { b"\x1b[C" }),
        "left" => Some(if app_cursor { b"\x1bOD" } else { b"\x1b[D" }),
        "home" => Some(if app_cursor { b"\x1bOH" } else { b"\x1b[H" }),
        "end" => Some(if app_cursor { b"\x1bOF" } else { b"\x1b[F" }),
        "pageup" => Some(b"\x1b[5~"),
        "pagedown" => Some(b"\x1b[6~"),
        "delete" => Some(b"\x1b[3~"),
        "insert" => Some(b"\x1b[2~"),
        _ => None,
    };
    if let Some(b) = named {
        return Some(b.to_vec());
    }

    // 4) Plain printable single char (no ctrl/alt/meta/cmd) -> its bytes.
    if !ks.ctrl && !ks.alt && !ks.meta && !ks.cmd && ks.key.chars().count() == 1 {
        return Some(ks.key.clone().into_bytes());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(key: &str, ctrl: bool, alt: bool) -> Keystroke {
        Keystroke {
            ctrl,
            alt,
            shift: false,
            cmd: false,
            meta: false,
            key: key.to_string(),
        }
    }

    #[test]
    fn enter_is_cr() {
        assert_eq!(keystroke_to_pty_bytes(&ks("enter", false, false), false), Some(b"\r".to_vec()));
    }

    #[test]
    fn backspace_is_del() {
        assert_eq!(keystroke_to_pty_bytes(&ks("backspace", false, false), false), Some(vec![0x7f]));
    }

    #[test]
    fn ctrl_c_is_etx() {
        assert_eq!(keystroke_to_pty_bytes(&ks("c", true, false), false), Some(vec![3]));
    }

    #[test]
    fn arrows_respect_app_cursor() {
        assert_eq!(keystroke_to_pty_bytes(&ks("up", false, false), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(keystroke_to_pty_bytes(&ks("up", false, false), true), Some(b"\x1bOA".to_vec()));
    }

    #[test]
    fn alt_left_is_word_back() {
        assert_eq!(keystroke_to_pty_bytes(&ks("left", false, true), false), Some(b"\x1bb".to_vec()));
    }

    #[test]
    fn plain_char() {
        assert_eq!(keystroke_to_pty_bytes(&ks("a", false, false), false), Some(b"a".to_vec()));
    }

    #[test]
    fn function_keys() {
        assert_eq!(keystroke_to_pty_bytes(&ks("f1", false, false), false), Some(b"\x1bOP".to_vec()));
        assert_eq!(keystroke_to_pty_bytes(&ks("f5", false, false), false), Some(b"\x1b[15~".to_vec()));
        assert_eq!(keystroke_to_pty_bytes(&ks("f12", false, false), false), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn ctrl_slash_is_us() {
        assert_eq!(keystroke_to_pty_bytes(&ks("/", true, false), false), Some(vec![0x1f]));
    }

    #[test]
    fn shift_tab_is_backtab() {
        let k = Keystroke {
            ctrl: false,
            alt: false,
            shift: true,
            cmd: false,
            meta: false,
            key: "tab".to_string(),
        };
        assert_eq!(keystroke_to_pty_bytes(&k, false), Some(b"\x1b[Z".to_vec()));
    }
}
