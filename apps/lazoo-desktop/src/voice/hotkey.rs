//! The global push-to-talk shortcut: registration from settings and press/release events.
use std::{str::FromStr, sync::atomic::Ordering};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use super::VoiceRuntime;

/// Result of the last registration attempt, shown in `VoiceState`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HotkeyStatus {
    pub registered: bool,
    pub error: Option<String>,
}

/// Plugin handler: forwards press and release to the `main` window, ignoring key auto-repeat
/// (a second press without a release) and stray releases.
pub fn on_event(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    let runtime = app.state::<VoiceRuntime>();
    let pressed = event.state() == ShortcutState::Pressed;
    if runtime.ptt_down.swap(pressed, Ordering::SeqCst) != pressed {
        let state = if pressed { "pressed" } else { "released" };
        let _ = app.emit_to("main", "voice://ptt", json!({ "state": state }));
    }
}

/// Replaces the registered shortcut with `hotkey` (empty disables it). Failure is recorded in
/// the status, never raised: another application may already own the chord.
pub fn apply(app: &AppHandle, hotkey: &str) {
    let runtime = app.state::<VoiceRuntime>();
    runtime.ptt_down.store(false, Ordering::SeqCst);
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    let status = if hotkey.is_empty() {
        HotkeyStatus::default()
    } else {
        match Shortcut::from_str(hotkey)
            .map_err(|error| error.to_string())
            .and_then(|shortcut| {
                shortcuts
                    .register(shortcut)
                    .map_err(|error| error.to_string())
            }) {
            Ok(()) => HotkeyStatus {
                registered: true,
                error: None,
            },
            Err(error) => HotkeyStatus {
                registered: false,
                error: Some(error),
            },
        }
    };
    *runtime.hotkey.lock().expect("hotkey status") = status;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    #[test]
    fn ui_hotkey_strings_parse_to_the_expected_chords() {
        let cases = [
            (
                "Ctrl+Alt+Space",
                Modifiers::CONTROL | Modifiers::ALT,
                Code::Space,
            ),
            (
                "Ctrl+Shift+F8",
                Modifiers::CONTROL | Modifiers::SHIFT,
                Code::F8,
            ),
            (
                "Alt+Super+Up",
                Modifiers::ALT | Modifiers::SUPER,
                Code::ArrowUp,
            ),
            (
                "Ctrl+Alt+Backquote",
                Modifiers::CONTROL | Modifiers::ALT,
                Code::Backquote,
            ),
            (
                "ctrl+alt+a",
                Modifiers::CONTROL | Modifiers::ALT,
                Code::KeyA,
            ),
            (
                "Ctrl+Shift+7",
                Modifiers::CONTROL | Modifiers::SHIFT,
                Code::Digit7,
            ),
            ("Alt+F24", Modifiers::ALT, Code::F24),
            ("Ctrl+Enter", Modifiers::CONTROL, Code::Enter),
            ("Ctrl+Tab", Modifiers::CONTROL, Code::Tab),
            (
                "Ctrl+Alt+Left",
                Modifiers::CONTROL | Modifiers::ALT,
                Code::ArrowLeft,
            ),
            (
                "Ctrl+Alt+Down",
                Modifiers::CONTROL | Modifiers::ALT,
                Code::ArrowDown,
            ),
            (
                "Ctrl+Alt+Right",
                Modifiers::CONTROL | Modifiers::ALT,
                Code::ArrowRight,
            ),
        ];
        for (text, modifiers, code) in cases {
            let parsed = Shortcut::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(parsed, Shortcut::new(Some(modifiers), code), "{text}");
        }
    }

    #[test]
    fn malformed_hotkey_strings_are_rejected() {
        for bad in [
            "Ctrl+Alt+",
            "Ctrl+",
            "Banana+X",
            "Ctrl+Alt+Space+Q",
            "+++",
            "Ctrl+Alt",
        ] {
            assert!(Shortcut::from_str(bad).is_err(), "{bad:?}");
        }
    }
}
