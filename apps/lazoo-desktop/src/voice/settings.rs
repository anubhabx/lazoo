//! Voice settings: types, validation, and the revision-checked JSON file under `voice/`.
use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use lazoo_runtime::{DataRoot, OwnerSecurity, atomic_write};
use serde::{Deserialize, Serialize};
use tauri_plugin_global_shortcut::Shortcut;

use super::{failure, whisper::WhisperModel};
use crate::client::DesktopFailure;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SttEngine {
    Off,
    WhisperLocal,
    Endpoint,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TtsEngine {
    Off,
    System,
    Endpoint,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cleaning {
    Off,
    Basic,
    Model,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DictationTarget {
    Insert,
    Clipboard,
    Conversation,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Stt {
    pub engine: SttEngine,
    pub model: WhisperModel,
    pub language: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Tts {
    pub engine: TtsEngine,
    pub voice: String,
    pub rate: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Endpoint {
    pub base_url: String,
    pub transcription_model: String,
    pub speech_model: String,
    pub speech_voice: String,
    pub cleanup_model: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct VoiceSettings {
    pub stt: Stt,
    pub tts: Tts,
    pub endpoint: Endpoint,
    pub cleaning: Cleaning,
    pub allow_external: bool,
    pub review_before_send: bool,
    pub read_replies: bool,
    pub hotkey: String,
    pub dictation_target: DictationTarget,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            stt: Stt {
                engine: SttEngine::WhisperLocal,
                model: WhisperModel::BaseEn,
                language: "en".into(),
            },
            tts: Tts {
                engine: TtsEngine::System,
                voice: String::new(),
                rate: 1.0,
            },
            endpoint: Endpoint {
                base_url: "https://api.openai.com/v1".into(),
                transcription_model: "gpt-4o-mini-transcribe".into(),
                speech_model: "gpt-4o-mini-tts".into(),
                speech_voice: "alloy".into(),
                cleanup_model: "gpt-4o-mini".into(),
            },
            cleaning: Cleaning::Basic,
            allow_external: false,
            review_before_send: true,
            read_replies: false,
            hotkey: "Ctrl+Alt+Space".into(),
            dictation_target: DictationTarget::Insert,
        }
    }
}

/// The file on disk: settings plus the monotonically increasing revision.
#[derive(Debug, Deserialize, Serialize)]
struct Stored {
    revision: u64,
    settings: VoiceSettings,
}

pub fn invalid(message: &str) -> DesktopFailure {
    failure("LZ-VOICE-SETTINGS", message)
}

/// Whether `host` (a URL host string, IPv6 in brackets) is `localhost`, `127.0.0.0/8` or `::1`.
fn host_is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback(),
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        Err(_) => false,
    }
}

/// Parses a base URL and says whether it is loopback. Returns `None` for anything that is not a
/// plain `http(s)://host[:port]/path` URL without credentials, query or fragment.
pub fn parse_base_url(raw: &str) -> Option<(reqwest::Url, bool)> {
    let url = reqwest::Url::parse(raw.trim()).ok()?;
    let host = url.host_str()?;
    let loopback = host_is_loopback(host);
    let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    let plain = url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    (scheme_ok && plain).then_some((url, loopback))
}

pub fn endpoint_is_external(settings: &VoiceSettings) -> bool {
    parse_base_url(&settings.endpoint.base_url).is_none_or(|(_, loopback)| !loopback)
}

fn text_field(label: &str, value: &str, required: bool) -> Result<(), DesktopFailure> {
    let trimmed = value.trim();
    if (required && trimmed.is_empty()) || value.len() > 200 || value.chars().any(char::is_control)
    {
        return Err(invalid(&format!("{label} is not valid.")));
    }
    Ok(())
}

/// Normalises (trims, drops a trailing slash from the base URL) and validates.
pub fn validate(mut settings: VoiceSettings) -> Result<VoiceSettings, DesktopFailure> {
    let language = settings.stt.language.trim().to_owned();
    if language != "auto"
        && !(language.len() == 2 && language.bytes().all(|byte| byte.is_ascii_lowercase()))
    {
        return Err(invalid(
            "The language must be \"auto\" or a two-letter ISO 639-1 code.",
        ));
    }
    settings.stt.language = language;

    if !(0.5..=2.0).contains(&settings.tts.rate) {
        return Err(invalid("The speech rate must be between 0.5 and 2.0."));
    }
    text_field("The voice name", &settings.tts.voice, false)?;

    let endpoint = &mut settings.endpoint;
    if parse_base_url(&endpoint.base_url).is_none() {
        return Err(invalid(
            "The endpoint base URL must be an https URL (http only for localhost) without credentials, query or fragment.",
        ));
    }
    endpoint.base_url = endpoint.base_url.trim().trim_end_matches('/').to_owned();
    for (label, value) in [
        ("The transcription model", &mut endpoint.transcription_model),
        ("The speech model", &mut endpoint.speech_model),
        ("The speech voice", &mut endpoint.speech_voice),
        ("The cleanup model", &mut endpoint.cleanup_model),
    ] {
        text_field(label, value, true)?;
        *value = value.trim().to_owned();
    }

    settings.hotkey = settings.hotkey.trim().to_owned();
    if !settings.hotkey.is_empty() && Shortcut::from_str(&settings.hotkey).is_err() {
        return Err(invalid("The push-to-talk hotkey cannot be parsed."));
    }
    Ok(settings)
}

pub fn voice_dir() -> Result<PathBuf, DesktopFailure> {
    let root = DataRoot::resolve().map_err(|_| {
        failure(
            "LZ-DESKTOP-RUNTIME",
            "The Lazoo data directory could not be resolved.",
        )
    })?;
    ensure_dir(&root.root().join("voice"))
}

/// Creates `path` and restricts it to the current user and SYSTEM.
pub fn ensure_dir(path: &Path) -> Result<PathBuf, DesktopFailure> {
    let unavailable = || {
        failure(
            "LZ-DESKTOP-RUNTIME",
            "The Lazoo voice directory could not be prepared.",
        )
    };
    fs::create_dir_all(path).map_err(|_| unavailable())?;
    OwnerSecurity::current()
        .and_then(|owner| owner.apply_to_path(path))
        .map_err(|_| unavailable())?;
    Ok(path.to_path_buf())
}

/// A missing or unreadable file yields the defaults at revision 0 (all safe: nothing external).
pub fn load() -> Result<(u64, VoiceSettings), DesktopFailure> {
    let path = voice_dir()?.join("settings.json");
    let stored = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok());
    Ok(stored.map_or_else(
        || (0, VoiceSettings::default()),
        |stored| (stored.revision, stored.settings),
    ))
}

/// Stores `settings` as revision `expected + 1`; `LZ-VOICE-REVISION` when the file moved on.
pub fn store(expected: u64, settings: VoiceSettings) -> Result<u64, DesktopFailure> {
    let (current, _) = load()?;
    if current != expected {
        return Err(failure(
            "LZ-VOICE-REVISION",
            "The voice settings changed elsewhere. Reload and apply your change again.",
        ));
    }
    let revision = current + 1;
    let bytes = serde_json::to_vec_pretty(&Stored { revision, settings })
        .map_err(|_| invalid("The voice settings could not be encoded."))?;
    atomic_write(&voice_dir()?.join("settings.json"), &bytes).map_err(|_| {
        failure(
            "LZ-DESKTOP-RUNTIME",
            "The voice settings could not be saved.",
        )
    })?;
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(change: impl FnOnce(&mut VoiceSettings)) -> Result<VoiceSettings, DesktopFailure> {
        let mut settings = VoiceSettings::default();
        change(&mut settings);
        validate(settings)
    }

    #[test]
    fn defaults_are_valid_and_match_the_contract() {
        let settings = validate(VoiceSettings::default()).expect("defaults validate");
        assert_eq!(settings, VoiceSettings::default());
        let json = serde_json::to_value(&settings).expect("json");
        assert_eq!(json["stt"]["model"], "base.en");
        assert_eq!(json["stt"]["engine"], "whisper-local");
        assert_eq!(json["endpoint"]["baseUrl"], "https://api.openai.com/v1");
        assert_eq!(json["allowExternal"], false);
        assert_eq!(json["dictationTarget"], "insert");
    }

    #[test]
    fn loopback_classification() {
        for host in [
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
            "127.8.9.10",
            "[::1]",
            "::1",
            "[::ffff:127.0.0.1]",
        ] {
            assert!(host_is_loopback(host), "{host}");
        }
        for host in [
            "api.openai.com",
            "localhost.evil.com",
            "192.168.1.5",
            "0.0.0.0",
            "[::2]",
            "10.0.0.1",
            "",
        ] {
            assert!(!host_is_loopback(host), "{host}");
        }
        assert!(parse_base_url("http://localhost:8080/v1").expect("url").1);
        assert!(parse_base_url("http://[::1]:9000/v1").expect("url").1);
        assert!(!parse_base_url("https://api.openai.com/v1").expect("url").1);
        assert!(
            parse_base_url("http://example.com/v1").is_none(),
            "plain http off loopback"
        );
        assert!(parse_base_url("https://user:pw@example.com/v1").is_none());
        assert!(parse_base_url("https://example.com/v1?key=1").is_none());
        assert!(parse_base_url("ftp://example.com").is_none());
        assert!(parse_base_url("not a url").is_none());
        assert!(!endpoint_is_external(
            &with(|s| s.endpoint.base_url = "http://127.0.0.1:8000/v1".into()).expect("valid")
        ));
        assert!(endpoint_is_external(&VoiceSettings::default()));
    }

    #[test]
    fn validation_rejects_bad_values_with_the_settings_code() {
        type Change = Box<dyn FnOnce(&mut VoiceSettings)>;
        let bad: Vec<Change> = vec![
            Box::new(|s| s.endpoint.base_url = "ftp://x".into()),
            Box::new(|s| s.endpoint.base_url = "http://example.com/v1".into()),
            Box::new(|s| s.tts.rate = 0.4),
            Box::new(|s| s.tts.rate = 2.1),
            Box::new(|s| s.tts.rate = f64::NAN),
            Box::new(|s| s.stt.language = "english".into()),
            Box::new(|s| s.stt.language = "EN".into()),
            Box::new(|s| s.endpoint.cleanup_model = "  ".into()),
            Box::new(|s| s.endpoint.speech_voice = "al\nloy".into()),
            Box::new(|s| s.hotkey = "Ctrl+Alt+".into()),
            Box::new(|s| s.hotkey = "Banana+X".into()),
        ];
        for change in bad {
            assert_eq!(with(change).expect_err("invalid").code, "LZ-VOICE-SETTINGS");
        }
    }

    #[test]
    fn validation_normalises_and_accepts_edge_values() {
        let settings = with(|s| {
            s.endpoint.base_url = " https://api.groq.com/openai/v1/ ".into();
            s.stt.language = "auto".into();
            s.tts.rate = 0.5;
            s.hotkey = String::new();
        })
        .expect("valid");
        assert_eq!(settings.endpoint.base_url, "https://api.groq.com/openai/v1");
        assert!(with(|s| s.tts.rate = 2.0).is_ok());
        assert!(with(|s| s.hotkey = "F9".into()).is_ok());
    }

    #[test]
    fn unknown_models_and_fields_do_not_deserialize() {
        let mut json = serde_json::to_value(VoiceSettings::default()).expect("json");
        json["stt"]["model"] = "huge".into();
        assert!(serde_json::from_value::<VoiceSettings>(json).is_err());
        let mut json = serde_json::to_value(VoiceSettings::default()).expect("json");
        json["extra"] = true.into();
        assert!(serde_json::from_value::<VoiceSettings>(json).is_err());
    }

    #[test]
    fn revision_conflicts_are_detected_and_files_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("lazoo-voice-settings-{}", uuid::Uuid::new_v4()));
        // SAFETY: this is the only test in the crate that touches LAZOO_DATA_ROOT.
        unsafe { std::env::set_var("LAZOO_DATA_ROOT", &dir) };

        assert_eq!(load().expect("defaults").0, 0);
        let changed = VoiceSettings {
            read_replies: true,
            ..VoiceSettings::default()
        };
        assert_eq!(store(0, changed.clone()).expect("first save"), 1);
        assert_eq!(
            store(0, changed.clone()).expect_err("stale").code,
            "LZ-VOICE-REVISION"
        );
        let (revision, loaded) = load().expect("load");
        assert_eq!((revision, loaded), (1, changed.clone()));
        assert_eq!(store(1, VoiceSettings::default()).expect("second save"), 2);

        let _ = fs::remove_dir_all(dir);
    }
}
