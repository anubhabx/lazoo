//! Types text into the focused Windows application with `SendInput` and Unicode key events.
use std::{
    mem::size_of,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{GlobalFree, HANDLE},
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
            KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN,
            VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT, VK_TAB,
        },
        WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
    },
};

use super::failure;
use crate::client::DesktopFailure;

fn refused(message: &str) -> DesktopFailure {
    failure("LZ-VOICE-INSERT-REFUSED", message)
}

/// Keyboard events for `text`: one key-down and key-up per UTF-16 unit (a surrogate pair is two
/// units), `\n` as Enter and `\t` as Tab. `\r\n` and a lone `\r` both count as one newline.
pub fn events(text: &str) -> Vec<INPUT> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let key = |vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let mut inputs = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        let (vk, scan, flags) = match unit {
            0x0A => (VK_RETURN, 0, KEYBD_EVENT_FLAGS(0)),
            0x09 => (VK_TAB, 0, KEYBD_EVENT_FLAGS(0)),
            _ => (VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE),
        };
        inputs.push(key(vk, scan, flags));
        inputs.push(key(vk, scan, flags | KEYEVENTF_KEYUP));
    }
    inputs
}

fn modifiers_down() -> bool {
    [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
        .into_iter()
        // The high bit of the async key state means the key is physically down right now.
        .any(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } < 0)
}

/// Types `text` into the foreground window. Refuses when no window has focus or the foreground
/// window belongs to this process (the front end then fills its own composer). Blocking: call it
/// from a blocking thread.
pub fn insert_text(text: &str) -> Result<(), DesktopFailure> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return Err(refused("No window has keyboard focus."));
    }
    let mut process = 0_u32;
    unsafe { GetWindowThreadProcessId(foreground, Some(&raw mut process)) };
    if process == std::process::id() {
        return Err(refused("Lazoo itself has focus."));
    }

    // The push-to-talk chord usually includes Ctrl or Alt; typing while they are held would turn
    // the text into shortcuts. Give the user up to a second to let go.
    let start = Instant::now();
    while modifiers_down() && start.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(20));
    }

    let inputs = events(text);
    let size = i32::try_from(size_of::<INPUT>()).expect("INPUT size");
    for batch in inputs.chunks(256) {
        let sent = unsafe { SendInput(batch, size) };
        if sent as usize != batch.len() {
            return Err(refused(
                "Windows did not accept the keyboard input, so the target may be elevated.",
            ));
        }
    }
    Ok(())
}

/// Puts `text` on the clipboard as `CF_UNICODETEXT`. Another application may hold the clipboard
/// open for a moment, so opening is retried briefly.
pub fn copy_text(text: &str) -> Result<(), DesktopFailure> {
    let failed = |message: &str| failure("LZ-VOICE-CLIPBOARD", message);
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: every Win32 call below follows its documented contract: the clipboard is opened
    // before use and always closed, the moveable block is locked only while it is filled, and
    // ownership passes to the system on a successful `SetClipboardData`.
    unsafe {
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        if !opened {
            return Err(failed("The clipboard is in use by another application."));
        }
        let result = (|| {
            EmptyClipboard().map_err(|_| failed("The clipboard could not be cleared."))?;
            let block = GlobalAlloc(GMEM_MOVEABLE, units.len() * size_of::<u16>())
                .map_err(|_| failed("Memory for the clipboard could not be allocated."))?;
            let target = GlobalLock(block);
            if target.is_null() {
                let _ = GlobalFree(Some(block));
                return Err(failed("Memory for the clipboard could not be locked."));
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), target.cast::<u16>(), units.len());
            let _ = GlobalUnlock(block);
            SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(block.0))).map_err(|_| {
                let _ = GlobalFree(Some(block));
                failed("The clipboard refused the text.")
            })?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(input: &INPUT) -> (u16, u16, u32) {
        let ki = unsafe { input.Anonymous.ki };
        (ki.wVk.0, ki.wScan, ki.dwFlags.0)
    }

    #[test]
    fn ascii_becomes_unicode_key_down_up_pairs() {
        let inputs = events("hi");
        assert_eq!(inputs.len(), 4);
        assert_eq!(unit(&inputs[0]), (0, u16::from(b'h'), KEYEVENTF_UNICODE.0));
        assert_eq!(
            unit(&inputs[1]),
            (0, u16::from(b'h'), KEYEVENTF_UNICODE.0 | KEYEVENTF_KEYUP.0)
        );
        assert_eq!(unit(&inputs[2]).1, u16::from(b'i'));
    }

    #[test]
    fn surrogate_pairs_are_two_units_and_newlines_are_enter() {
        // U+1F600 is the surrogate pair D83D DE00.
        let inputs = events("\u{1f600}");
        assert_eq!(inputs.len(), 4);
        assert_eq!(unit(&inputs[0]).1, 0xD83D);
        assert_eq!(unit(&inputs[2]).1, 0xDE00);

        let inputs = events("a\r\nb\rc\n");
        let enters = inputs
            .iter()
            .filter(|i| unit(i).0 == VK_RETURN.0 && unit(i).2 & KEYEVENTF_KEYUP.0 == 0)
            .count();
        assert_eq!(enters, 3);
        assert_eq!(
            unit(&inputs[2]),
            (VK_RETURN.0, 0, 0),
            "Enter is a virtual key, not a Unicode packet"
        );
        assert!(events("").is_empty());
    }

    #[test]
    #[ignore = "overwrites the real clipboard; run with --ignored"]
    fn clipboard_round_trips_unicode_text() {
        let text = "Lazoo voice \u{e9}\u{1f600} test";
        copy_text(text).expect("copy");
        let read = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-Clipboard -Raw",
            ])
            .output()
            .expect("powershell");
        assert_eq!(String::from_utf8_lossy(&read.stdout).trim(), text);
    }
}
