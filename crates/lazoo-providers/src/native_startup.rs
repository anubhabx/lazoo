//! Startup-only native trust recognition. The caller owns consent; these bytes are never turn truth.
use crate::{ProviderError, ProviderKind};
use lazoo_runtime::TerminalOutput;
use std::path::Path;
const MAX_SCREEN: usize = 64 * 1024;
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StartupState {
    Unknown,
    TrustPrompt {
        input: Vec<u8>,
    },
    /// The native trust dialog shows Yes selected; Enter accepts it.
    TrustSelected,
    Ready,
}
#[derive(Default)]
pub(crate) struct NativeStartupScreen {
    bytes: Vec<u8>,
}
impl NativeStartupScreen {
    pub(crate) fn consume(
        &mut self,
        provider: ProviderKind,
        cwd: &Path,
        output: &TerminalOutput,
    ) -> Result<StartupState, ProviderError> {
        if output.gap {
            return Err(protocol("native startup output was lost"));
        }
        self.bytes.extend_from_slice(&output.bytes);
        if let Some(index) = self.bytes.windows(4).rposition(|part| part == b"\x1b[2J") {
            self.bytes.drain(..index + 4);
        }
        if self.bytes.len() > MAX_SCREEN {
            return Err(protocol("native startup screen exceeded bound"));
        }
        poll_screen(provider, cwd, &self.bytes)
    }
}
fn protocol(message: &str) -> ProviderError {
    ProviderError::Protocol(message.to_owned())
}
fn path_text(value: &str) -> String {
    value
        .trim()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .to_ascii_lowercase()
}
pub(crate) fn poll_screen(
    provider: ProviderKind,
    cwd: &Path,
    bytes: &[u8],
) -> Result<StartupState, ProviderError> {
    if bytes.len() > MAX_SCREEN {
        return Err(protocol("native startup screen exceeded bound"));
    }
    let screen = normalize(bytes);
    if provider == ProviderKind::Antigravity {
        if [
            "Do you trust the contents of this project?",
            "Yes, I trust this folder",
            "No, exit",
        ]
        .iter()
        .all(|marker| screen.contains(marker))
        {
            let canonical = path_text(&cwd.canonicalize()?.to_string_lossy());
            // The native dialog can retain the application header behind it. Require the
            // exact displayed workspace, including boundaries, before selecting its Yes key.
            let normalized = format!(" {} ", path_text(&screen));
            if !normalized.contains(&format!(" {canonical} ")) {
                return Err(protocol("native trust screen showed a foreign workspace"));
            }
            return Ok(StartupState::TrustPrompt {
                input: b"y".to_vec(),
            });
        }
        if screen.contains("Antigravity CLI") && screen.contains("? for shortcuts") {
            return Ok(StartupState::Ready);
        }
        return Ok(StartupState::Unknown);
    }
    let pointer = claude_trust_pointer(bytes);
    if provider != ProviderKind::ClaudeCode
        || pointer.is_none()
        || ![
            "Quick safety check",
            "Yes, I trust this folder",
            "Enter to confirm",
        ]
        .iter()
        .all(|marker| screen.contains(marker))
    {
        return Ok(StartupState::Unknown);
    }
    let displayed = screen
        .split_once("Accessing workspace:")
        .and_then(|(_, tail)| tail.split_once("Quick safety check").map(|(path, _)| path))
        .ok_or_else(|| protocol("native trust workspace is ambiguous"))?;
    let canonical = cwd.canonicalize()?;
    // A workspace path wider than the terminal wraps onto the next row, and that cursor move
    // normalizes to a space. ponytail: whitespace is ignored on both sides, so paths that
    // differ only in spacing compare equal; parse rows if that ever matters.
    let compact = |value: &str| path_text(value).split_whitespace().collect::<String>();
    if compact(displayed) != compact(&canonical.to_string_lossy()) {
        return Err(protocol("native trust screen showed a foreign workspace"));
    }
    // The native screen defaults to No. Select Yes; the caller confirms only once Yes shows.
    if pointer == Some(TrustOption::Yes) {
        return Ok(StartupState::TrustSelected);
    }
    Ok(StartupState::TrustPrompt {
        input: b"\x1b[B".to_vec(),
    })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrustOption {
    No,
    Yes,
}
/// The option under Claude Code's latest selection pointer. A moved selection is redrawn as
/// bare glyphs at absolute positions (`CSI 14;2H` space, `CSI 15;2H` pointer), so the rows of
/// the labels and of the last pointer glyph decide it, not the label text next to a pointer.
/// Claude Code 2.1.316 draws the pointer as U+276F; older builds drew ">".
fn claude_trust_pointer(bytes: &[u8]) -> Option<TrustOption> {
    let mut rows: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
    let mut row = 1_usize;
    let mut pointer_row = None;
    let mut chars = String::from_utf8_lossy(bytes)
        .chars()
        .collect::<Vec<_>>()
        .into_iter();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let command = chars.by_ref().find(|p| {
                        let end = ('\u{40}'..='\u{7e}').contains(p);
                        if !end {
                            params.push(*p);
                        }
                        end
                    });
                    let count = params
                        .split(';')
                        .next()
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(1)
                        .max(1);
                    match command {
                        Some('H' | 'f') => row = count,
                        Some('A' | 'F') => row = row.saturating_sub(count).max(1),
                        Some('B' | 'E') => row += count,
                        Some('C') => rows.entry(row).or_default().push(' '),
                        _ => {}
                    }
                }
                // Operating system commands end at BEL or ST.
                Some(']') => {
                    while let Some(p) = chars.next() {
                        if p == '\u{7}' || (p == '\u{1b}' && chars.next() == Some('\\')) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' => row += 1,
            '\u{276f}' | '>' => {
                pointer_row = Some(row);
                rows.entry(row).or_default().push(c);
            }
            c if !c.is_control() => rows.entry(row).or_default().push(c),
            _ => {}
        }
    }
    let row_of = |label: &str| {
        rows.iter()
            .find(|(_, text)| text.contains(label))
            .map(|(row, _)| *row)
    };
    let pointer_row = pointer_row?;
    if row_of("Yes, I trust this folder") == Some(pointer_row) {
        Some(TrustOption::Yes)
    } else if row_of("No, exit") == Some(pointer_row) {
        Some(TrustOption::No)
    } else {
        None
    }
}
fn normalize(bytes: &[u8]) -> String {
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        match bytes.get(index) {
            Some(b'[') => {
                index += 1;
                while index < bytes.len() && !(0x40..=0x7e).contains(&bytes[index]) {
                    index += 1;
                }
                if index == bytes.len() {
                    break;
                }
                let command = bytes[index];
                if matches!(
                    command,
                    b'C' | b'H' | b'f' | b'G' | b'A' | b'B' | b'E' | b'F'
                ) {
                    output.push(b' ');
                }
                // Formatting/erase commands add no literal text.
                index += 1;
            }
            Some(b']') => {
                index += 1;
                while index < bytes.len()
                    && bytes[index] != 7
                    && !(bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\'))
                {
                    index += 1;
                }
                if index < bytes.len() {
                    index += if bytes[index] == 7 { 1 } else { 2 };
                }
            }
            Some(_) => {
                index += 1;
            }
            None => break,
        }
    }
    String::from_utf8_lossy(&output)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
#[cfg(test)]
mod tests {
    use super::{NativeStartupScreen, StartupState, poll_screen};
    use crate::ProviderKind;
    use lazoo_runtime::TerminalOutput;
    #[test]
    fn framed_claude_trust_selects_yes_and_rejects_foreign_prefix() {
        let cwd = tempfile::tempdir().unwrap();
        let raw = format!(
            "\x1b[2J\x1b[3;2HAccessing\x1b[1Cworkspace:\x1b[5;2H{}\x1b[7;2HQuick\x1b[1Csafety\x1b[1Ccheck:\x1b[14;2H>\x1b[1CNo,\x1b[1Cexit\x1b[15;4HYes,\x1b[1CI\x1b[1Ctrust\x1b[1Cthis\x1b[1Cfolder\x1b[17;2HEnter\x1b[1Cto\x1b[1Cconfirm",
            cwd.path().display()
        );
        assert_eq!(
            poll_screen(ProviderKind::ClaudeCode, cwd.path(), raw.as_bytes()).unwrap(),
            StartupState::TrustPrompt {
                input: b"\x1b[B".to_vec()
            }
        );
        let foreign = raw.replace(
            &cwd.path().display().to_string(),
            &format!("{}-foreign", cwd.path().display()),
        );
        assert!(poll_screen(ProviderKind::ClaudeCode, cwd.path(), foreign.as_bytes()).is_err());
        // A long task worktree path wraps onto the next row of the 120-column terminal.
        let shown = cwd.path().display().to_string();
        let (head, tail) = shown.split_at(shown.len() / 2);
        let wrapped = raw.replace(&shown, &format!("{head}\x1b[6;2H{tail}"));
        assert!(matches!(
            poll_screen(ProviderKind::ClaudeCode, cwd.path(), wrapped.as_bytes()).unwrap(),
            StartupState::TrustPrompt { .. }
        ));
        let heavy_pointer = raw.replace("\x1b[14;2H>", "\x1b[14;2H\u{276f}");
        assert_eq!(
            poll_screen(
                ProviderKind::ClaudeCode,
                cwd.path(),
                heavy_pointer.as_bytes()
            )
            .unwrap(),
            StartupState::TrustPrompt {
                input: b"\x1b[B".to_vec()
            }
        );
        let pointer_on_yes = raw.replace("\x1b[14;2H>", "\x1b[14;2H ");
        assert_eq!(
            poll_screen(
                ProviderKind::ClaudeCode,
                cwd.path(),
                pointer_on_yes.as_bytes()
            )
            .unwrap(),
            StartupState::Unknown
        );
        // Claude 2.1.316 redraws only the two option rows after Down; the latest pointer wins,
        // and only a screen showing Yes selected is confirmed.
        let moved = format!(
            "{heavy_pointer}\x1b[14;2H \x1b[1CNo,\x1b[1Cexit\x1b[15;2H\u{276f}\x1b[1CYes,\x1b[1CI\x1b[1Ctrust\x1b[1Cthis\x1b[1Cfolder"
        );
        assert_eq!(
            poll_screen(ProviderKind::ClaudeCode, cwd.path(), moved.as_bytes()).unwrap(),
            StartupState::TrustSelected
        );
        let back = format!(
            "{moved}\x1b[14;2H\u{276f}\x1b[1CNo,\x1b[1Cexit\x1b[15;2H \x1b[1CYes,\x1b[1CI\x1b[1Ctrust\x1b[1Cthis\x1b[1Cfolder"
        );
        assert!(matches!(
            poll_screen(ProviderKind::ClaudeCode, cwd.path(), back.as_bytes()).unwrap(),
            StartupState::TrustPrompt { .. }
        ));
        // What Claude Code 2.1.316 actually writes after Down: bare glyphs at absolute rows.
        let bare_yes = format!(
            "{heavy_pointer}\x1b[?2026h\x1b[?2026l\x1b[14;2H \x1b[15;2H\u{276f}\x1b[?2026h\x1b[?2026l"
        );
        assert_eq!(
            poll_screen(ProviderKind::ClaudeCode, cwd.path(), bare_yes.as_bytes()).unwrap(),
            StartupState::TrustSelected
        );
        let bare_no = format!("{bare_yes}\x1b[14;2H\u{276f}\x1b[15;2H ");
        assert!(matches!(
            poll_screen(ProviderKind::ClaudeCode, cwd.path(), bare_no.as_bytes()).unwrap(),
            StartupState::TrustPrompt { .. }
        ));
        let mut parser = NativeStartupScreen::default();
        for (index, byte) in raw.bytes().enumerate() {
            let state = parser
                .consume(
                    ProviderKind::ClaudeCode,
                    cwd.path(),
                    &TerminalOutput {
                        start: index as u64,
                        end: index as u64 + 1,
                        bytes: vec![byte],
                        gap: false,
                    },
                )
                .unwrap();
            if index == raw.len() - 1 {
                assert!(matches!(state, StartupState::TrustPrompt { .. }));
            }
        }
    }
    #[test]
    fn antigravity_ready_is_native_prompt_not_sign_in() {
        let cwd = std::env::temp_dir();
        assert_eq!(
            poll_screen(
                ProviderKind::Antigravity,
                &cwd,
                b"Welcome to Antigravity CLI Signing in..."
            )
            .unwrap(),
            StartupState::Unknown
        );
        assert_eq!(
            poll_screen(
                ProviderKind::Antigravity,
                &cwd,
                b"Antigravity CLI 1.4.9 > ? for shortcuts"
            )
            .unwrap(),
            StartupState::Ready
        );
    }
    #[test]
    fn antigravity_trust_requires_exact_workspace_before_ready() {
        let cwd = tempfile::tempdir().unwrap();
        let screen = format!(
            "Antigravity CLI 1.4.9\r\n{}\r\n? for shortcuts\r\nDo you trust the contents of this project?\r\nYes, I trust this folder\r\nNo, exit",
            cwd.path().display()
        );
        assert_eq!(
            poll_screen(ProviderKind::Antigravity, cwd.path(), screen.as_bytes()).unwrap(),
            StartupState::TrustPrompt {
                input: b"y".to_vec()
            }
        );
        let foreign = screen.replace(
            &cwd.path().display().to_string(),
            &format!("{}-foreign", cwd.path().display()),
        );
        assert!(poll_screen(ProviderKind::Antigravity, cwd.path(), foreign.as_bytes()).is_err());
    }
}
