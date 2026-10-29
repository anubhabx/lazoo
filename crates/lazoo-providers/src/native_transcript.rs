//! Bounded, turn-scoped Antigravity transcript reads. Native screen bytes are not model text.

use crate::ProviderError;
use serde_json::Value;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MAX_TURN_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 64 * 1024;

pub(super) struct TranscriptBinding {
    artifact: PathBuf,
    path: PathBuf,
    cursor: u64,
}

impl TranscriptBinding {
    pub(super) fn from_native_id(id: &str) -> Result<Self, ProviderError> {
        uuid::Uuid::parse_str(id).map_err(|_| protocol("invalid Antigravity conversation id"))?;
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .ok_or_else(|| protocol("native home is unavailable"))?;
        let root = fs::canonicalize(PathBuf::from(home).join(".gemini/antigravity-cli/brain"))?;
        let artifact = fs::canonicalize(root.join(id))?;
        if artifact.parent() != Some(root.as_path()) {
            return Err(protocol(
                "native artifact directory escapes conversation root",
            ));
        }
        Ok(Self {
            path: artifact.join(".system_generated/logs/transcript_full.jsonl"),
            artifact,
            cursor: 0,
        })
    }

    pub(super) fn validate_paths(&self, payload: &Value) -> Result<(), ProviderError> {
        let artifact = payload
            .get("artifactDirectoryPath")
            .and_then(Value::as_str)
            .ok_or_else(|| protocol("Antigravity hook omitted artifact directory"))?;
        let transcript = payload
            .get("transcriptPath")
            .and_then(Value::as_str)
            .ok_or_else(|| protocol("Antigravity hook omitted transcript path"))?;
        if fs::canonicalize(artifact)? != self.artifact
            || normalized_path(Path::new(transcript)) != normalized_path(&self.path)
        {
            return Err(protocol("Antigravity transcript path drifted"));
        }
        Ok(())
    }

    pub(super) fn begin_turn(&mut self) -> Result<(), ProviderError> {
        self.cursor = match fs::metadata(&self.path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        Ok(())
    }

    pub(super) fn text(&self, prompt: &str) -> Result<Option<String>, ProviderError> {
        let canonical = fs::canonicalize(&self.path)?;
        if !canonical.starts_with(&self.artifact)
            || normalized_path(&canonical) != normalized_path(&self.path)
        {
            return Err(protocol("native transcript escapes its bound conversation"));
        }
        let mut file = fs::File::open(canonical)?;
        let end = file.metadata()?.len();
        let length = end
            .checked_sub(self.cursor)
            .filter(|value| *value <= MAX_TURN_BYTES)
            .ok_or_else(|| protocol("native transcript was truncated or exceeds turn bound"))?;
        file.seek(SeekFrom::Start(self.cursor))?;
        let mut bytes = Vec::new();
        file.take(length).read_to_end(&mut bytes)?;
        parse_turn(&bytes, prompt)
    }
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches("\\\\?\\")
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn protocol(message: &str) -> ProviderError {
    ProviderError::Protocol(message.to_owned())
}

fn parse_turn(bytes: &[u8], prompt: &str) -> Result<Option<String>, ProviderError> {
    let raw = std::str::from_utf8(bytes).map_err(|_| protocol("native transcript is not UTF-8"))?;
    let mut saw_user = false;
    let mut last_step = None;
    let mut text = String::new();
    let mut final_assistant = false;
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        if line.len() > MAX_LINE_BYTES {
            return Err(protocol("native transcript row exceeds bound"));
        }
        let row: Value = serde_json::from_str(line)?;
        let step = row
            .get("step_index")
            .and_then(Value::as_u64)
            .ok_or_else(|| protocol("native transcript step identity missing"))?;
        if last_step.is_some_and(|prior| step <= prior) {
            return Err(protocol("native transcript steps replayed or reordered"));
        }
        last_step = Some(step);
        let source = row.get("source").and_then(Value::as_str);
        let kind = row.get("type").and_then(Value::as_str);
        if source == Some("USER_EXPLICIT") && kind == Some("USER_INPUT") {
            let content = row
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| protocol("native transcript user content missing"))?;
            let user = content
                .strip_prefix("<USER_REQUEST>\n")
                .and_then(|s| s.split_once("\n</USER_REQUEST>").map(|(user, _)| user))
                .unwrap_or(content);
            if saw_user || user != prompt {
                return Err(protocol(
                    "native transcript does not match the canonical turn",
                ));
            }
            saw_user = true;
        } else if source == Some("MODEL")
            && kind == Some("PLANNER_RESPONSE")
            && row.get("status").and_then(Value::as_str) == Some("DONE")
        {
            if !saw_user {
                return Err(protocol(
                    "native transcript response preceded canonical input",
                ));
            }
            final_assistant = row
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty);
            if let Some(content) = row.get("content").and_then(Value::as_str) {
                if text.len().saturating_add(content.len()) > MAX_TEXT_BYTES {
                    return Err(protocol("native transcript response exceeds bound"));
                }
                text.push_str(content);
            }
        }
        // GENERIC rows include command/tool results, not assistant responses. Never publish
        // the provider's hidden `thinking` field or old session history as a new text delta.
    }
    if !saw_user {
        return Err(protocol(
            "native transcript has not recorded the canonical input",
        ));
    }
    if !final_assistant {
        return Err(protocol(
            "native transcript has no completed assistant response for this turn",
        ));
    }
    Ok((!text.is_empty()).then_some(text))
}

#[cfg(test)]
mod tests {
    use super::parse_turn;
    use serde_json::json;
    fn rows(values: &[serde_json::Value]) -> Vec<u8> {
        use std::fmt::Write;
        let mut text = String::new();
        for value in values {
            writeln!(&mut text, "{value}").unwrap();
        }
        text.into_bytes()
    }
    #[test]
    fn only_current_assistant_content_is_published() {
        let bytes = rows(&[
            json!({"step_index":8,"source":"USER_EXPLICIT","type":"USER_INPUT","content":"<USER_REQUEST>\nhello\n</USER_REQUEST>\nmetadata"}),
            json!({"step_index":9,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","thinking":"private","tool_calls":[]}),
            json!({"step_index":10,"source":"MODEL","type":"GENERIC","status":"DONE","content":"tool output"}),
            json!({"step_index":11,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"assistant reply"}),
        ]);
        assert_eq!(
            parse_turn(&bytes, "hello").unwrap().as_deref(),
            Some("assistant reply")
        );
        assert!(parse_turn(&bytes, "foreign").is_err());
    }
    #[test]
    fn old_input_replay_and_missing_input_fail_closed() {
        let user =
            json!({"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","content":"hello"});
        assert!(parse_turn(&rows(&[user.clone(), user]), "hello").is_err());
        assert!(parse_turn(&rows(&[json!({"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"old"})]),"hello").is_err());
    }
    #[test]
    fn delayed_stop_cannot_complete_input_only_or_pending_tools() {
        let user = json!({"step_index":10,"source":"USER_EXPLICIT","type":"USER_INPUT","content":"same prompt"});
        assert!(parse_turn(&rows(std::slice::from_ref(&user)), "same prompt").is_err());
        let pending = json!({"step_index":11,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"running","tool_calls":[{"name":"run_command"}]});
        assert!(parse_turn(&rows(&[user, pending]), "same prompt").is_err());
    }
}
