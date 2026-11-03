//! Lazoo Voice (desktop side): settings, local and endpoint speech engines, cleaning, text
//! insertion, the push-to-talk hotkey and the overlay window. See
//! `docs/superpowers/specs/2026-11-04-voice-mode-design.md`.
//!
//! Privacy rules enforced here: audio exists only in memory or in a temporary owner-protected WAV
//! that is always deleted; nothing external is contacted unless `allowExternal` is on; nothing
//! (audio, transcript text, key) is ever logged.
pub mod clean;
mod endpoint;
pub mod hotkey;
mod insert;
mod settings;
pub mod shell;
mod whisper;

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{
    AppHandle, Emitter, Manager, State,
    ipc::{Channel, InvokeBody, Request, Response},
};

use crate::client::DesktopFailure;
use endpoint::Target;
use hotkey::HotkeyStatus;
use settings::{
    Cleaning, SttEngine, TtsEngine, VoiceSettings, endpoint_is_external, invalid, voice_dir,
};
use whisper::{Progress, WhisperModel, WhisperState};

pub fn failure(code: &str, message: &str) -> DesktopFailure {
    DesktopFailure::new(code, message)
}

/// Runtime flags shared by the commands and the hotkey handler.
#[derive(Default)]
pub struct VoiceRuntime {
    installing: AtomicBool,
    overlay_visible: AtomicBool,
    pub(crate) ptt_down: AtomicBool,
    pub(crate) hotkey: Mutex<HotkeyStatus>,
}

struct InstallGuard<'a>(&'a AtomicBool);

impl Drop for InstallGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl VoiceRuntime {
    fn begin_install(&self) -> Result<InstallGuard<'_>, DesktopFailure> {
        self.installing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| InstallGuard(&self.installing))
            .map_err(|_| {
                failure(
                    "LZ-VOICE-BUSY",
                    "A voice engine install or removal is already running.",
                )
            })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // A wire snapshot for the UI (spec section 4), not a state machine.
pub struct VoiceState {
    revision: u64,
    settings: VoiceSettings,
    key_set: bool,
    endpoint_external: bool,
    whisper: WhisperState,
    hotkey_registered: bool,
    hotkey_error: Option<String>,
    microphone_blocked: bool,
}

fn state_of(runtime: &VoiceRuntime) -> Result<VoiceState, DesktopFailure> {
    let (revision, settings) = settings::load()?;
    let voice = voice_dir()?;
    let hotkey = runtime.hotkey.lock().expect("hotkey status").clone();
    Ok(VoiceState {
        revision,
        key_set: endpoint::key_is_set(&voice),
        endpoint_external: endpoint_is_external(&settings),
        whisper: whisper::state(&voice),
        hotkey_registered: hotkey.registered,
        hotkey_error: hotkey.error,
        microphone_blocked: shell::microphone_blocked(),
        settings,
    })
}

#[tauri::command]
pub async fn voice_open_microphone_settings() -> Result<(), DesktopFailure> {
    shell::open_microphone_settings().map_err(|_| {
        failure(
            "LZ-VOICE-SETTINGS-OPEN",
            "Windows Settings could not be opened. Open Privacy & security > Microphone yourself.",
        )
    })
}

/// Startup work: hidden overlay window, temp sweep, hotkey registration, microphone permission.
pub fn setup(app: &tauri::App) -> tauri::Result<()> {
    let handle = app.handle();
    if let Ok(voice) = voice_dir() {
        whisper::sweep_tmp(&voice);
    }
    shell::create_overlay(handle)?;
    let hotkey = settings::load()
        .map(|(_, settings)| settings.hotkey)
        .unwrap_or_default();
    hotkey::apply(handle, &hotkey);
    #[cfg(windows)]
    if let Some(main) = handle.get_webview_window("main") {
        shell::allow_microphone(&main);
    }
    Ok(())
}

#[tauri::command]
pub async fn voice_state(runtime: State<'_, VoiceRuntime>) -> Result<VoiceState, DesktopFailure> {
    state_of(&runtime)
}

#[tauri::command]
pub async fn voice_settings_set(
    app: AppHandle,
    runtime: State<'_, VoiceRuntime>,
    settings: Value,
    revision: u64,
) -> Result<VoiceState, DesktopFailure> {
    let parsed = serde_json::from_value::<VoiceSettings>(settings)
        .map_err(|_| invalid("The voice settings have an unknown field or an unknown value."))?;
    let valid = settings::validate(parsed)?;
    let hotkey = valid.hotkey.clone();
    settings::store(revision, valid)?;
    hotkey::apply(&app, &hotkey);
    state_of(&runtime)
}

#[tauri::command]
pub async fn voice_key_set(
    runtime: State<'_, VoiceRuntime>,
    key: String,
) -> Result<VoiceState, DesktopFailure> {
    endpoint::set_key(&voice_dir()?, &key)?;
    state_of(&runtime)
}

#[tauri::command]
pub async fn voice_key_clear(
    runtime: State<'_, VoiceRuntime>,
) -> Result<VoiceState, DesktopFailure> {
    endpoint::clear_key(&voice_dir()?);
    state_of(&runtime)
}

fn parse_model(name: &str) -> Result<WhisperModel, DesktopFailure> {
    WhisperModel::parse(name).ok_or_else(|| invalid("That is not a known whisper model."))
}

#[tauri::command]
pub async fn voice_whisper_install(
    runtime: State<'_, VoiceRuntime>,
    model: String,
    on_progress: Channel<Progress>,
) -> Result<VoiceState, DesktopFailure> {
    let model = parse_model(&model)?;
    let _guard = runtime.begin_install()?;
    whisper::install(&voice_dir()?, model, &on_progress).await?;
    state_of(&runtime)
}

#[tauri::command]
pub async fn voice_whisper_remove(
    runtime: State<'_, VoiceRuntime>,
    model: Option<String>,
) -> Result<VoiceState, DesktopFailure> {
    let model = model.as_deref().map(parse_model).transpose()?;
    let _guard = runtime.begin_install()?;
    whisper::remove(&voice_dir()?, model);
    state_of(&runtime)
}

const SAMPLE_RATE: u32 = 16_000;
const MAX_AUDIO_BYTES: usize = 5 * 60 * SAMPLE_RATE as usize * 2;

fn audio_error(message: &str) -> DesktopFailure {
    failure("LZ-VOICE-AUDIO", message)
}

/// Checks RIFF/WAVE, PCM, mono, 16 kHz, 16-bit, non-empty and at most five minutes.
/// Returns the audio length in milliseconds.
pub fn validate_wav(bytes: &[u8]) -> Result<u64, DesktopFailure> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(audio_error("The audio is not a WAV file."));
    }
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let half = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let (mut position, mut format_ok, mut data_len) = (12_usize, false, None);
    while position + 8 <= bytes.len() {
        let size = word(position + 4) as usize;
        let body = position + 8;
        match &bytes[position..position + 4] {
            b"fmt " => {
                if size < 16 || body + 16 > bytes.len() {
                    return Err(audio_error("The WAV format chunk is damaged."));
                }
                format_ok = half(body) == 1
                    && half(body + 2) == 1
                    && word(body + 4) == SAMPLE_RATE
                    && half(body + 14) == 16;
            }
            b"data" => {
                data_len = Some(size.min(bytes.len() - body));
                break;
            }
            _ => {}
        }
        position = body.saturating_add(size).saturating_add(size & 1);
    }
    if !format_ok {
        return Err(audio_error("The audio must be PCM 16-bit mono at 16 kHz."));
    }
    let data_len = data_len.unwrap_or(0);
    if data_len < 2 {
        return Err(audio_error("The audio is empty."));
    }
    if data_len > MAX_AUDIO_BYTES {
        return Err(audio_error("The audio is longer than five minutes."));
    }
    Ok(data_len as u64 / 32)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    text: String,
    raw: String,
    cleaning: Cleaning,
    engine: SttEngine,
    /// True when audio or text was sent to a non-loopback endpoint for this request.
    external: bool,
    audio_ms: u64,
    elapsed_ms: u64,
}

/// `text` is `raw` after cleaning. Model cleaning that is refused, unconfigured or failing falls
/// back to basic cleaning, and the returned `cleaning` says which one actually ran.
async fn run_cleaning(
    voice: &std::path::Path,
    settings: &VoiceSettings,
    raw: &str,
    external: &mut bool,
) -> (String, Cleaning) {
    if raw.is_empty() {
        let used = if settings.cleaning == Cleaning::Model {
            Cleaning::Basic
        } else {
            settings.cleaning
        };
        return (String::new(), used);
    }
    match settings.cleaning {
        Cleaning::Off => (raw.to_owned(), Cleaning::Off),
        Cleaning::Basic => (clean::basic(raw), Cleaning::Basic),
        Cleaning::Model => {
            if let Ok(target) = Target::resolve(voice, settings) {
                *external |= target.external;
                if let Ok(cleaned) = target.clean(settings, raw).await {
                    return (cleaned, Cleaning::Model);
                }
            }
            (clean::basic(raw), Cleaning::Basic)
        }
    }
}

/// The request body is the raw WAV; the `x-lazoo-purpose` header (`conversation` or `dictation`)
/// is informational and does not change behaviour.
#[tauri::command]
pub async fn voice_transcribe(request: Request<'_>) -> Result<Transcript, DesktopFailure> {
    let InvokeBody::Raw(wav) = request.body() else {
        return Err(audio_error("The audio must be sent as a raw WAV body."));
    };
    transcribe_wav(wav).await
}

async fn transcribe_wav(wav: &[u8]) -> Result<Transcript, DesktopFailure> {
    let started = Instant::now();
    let (_, settings) = settings::load()?;
    let engine = settings.stt.engine;
    if engine == SttEngine::Off {
        return Err(failure(
            "LZ-VOICE-OFF",
            "Speech to text is turned off in Settings, Voice.",
        ));
    }
    let audio_ms = validate_wav(wav)?;
    let voice = voice_dir()?;
    let mut external = false;
    let raw = if engine == SttEngine::Endpoint {
        let target = Target::resolve(&voice, &settings)?;
        external = target.external;
        target.transcribe(&settings, wav).await?
    } else {
        whisper::transcribe(&voice, settings.stt.model, &settings.stt.language, wav).await?
    };
    let raw = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let (text, cleaning) = run_cleaning(&voice, &settings, &raw, &mut external).await;
    Ok(Transcript {
        text,
        raw,
        cleaning,
        engine,
        external,
        audio_ms,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

#[tauri::command]
pub async fn voice_speak(text: String) -> Result<Response, DesktopFailure> {
    let (_, settings) = settings::load()?;
    if settings.tts.engine != TtsEngine::Endpoint {
        return Err(failure(
            "LZ-VOICE-OFF",
            "Endpoint speech is not selected in Settings, Voice.",
        ));
    }
    let text: String = text.chars().take(4096).collect();
    if text.trim().is_empty() {
        return Err(invalid("There is no text to speak."));
    }
    let target = Target::resolve(&voice_dir()?, &settings)?;
    Ok(Response::new(target.speak(&settings, &text).await?))
}

#[tauri::command]
pub async fn voice_insert_text(text: String) -> Result<(), DesktopFailure> {
    tokio::task::spawn_blocking(move || insert::insert_text(&text))
        .await
        .map_err(|_| failure("LZ-VOICE-INSERT-REFUSED", "Text insertion was interrupted."))?
}

/// Writes `text` to the Windows clipboard from Rust, because the web view's clipboard API
/// refuses writes while the main window is not focused (the global push-to-talk case).
#[tauri::command]
pub async fn voice_copy_text(text: String) -> Result<(), DesktopFailure> {
    tokio::task::spawn_blocking(move || insert::copy_text(&text))
        .await
        .map_err(|_| failure("LZ-VOICE-CLIPBOARD", "The clipboard write was interrupted."))?
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayState {
    Hidden,
    Recording,
    Transcribing,
    Done,
    Error,
}

/// Shows or hides the overlay (never focusing it) and forwards the state to its web view.
#[tauri::command]
pub async fn voice_overlay(
    app: AppHandle,
    runtime: State<'_, VoiceRuntime>,
    state: OverlayState,
    detail: Option<String>,
    level: Option<f64>,
) -> Result<(), DesktopFailure> {
    let window = app.get_webview_window(shell::OVERLAY).ok_or_else(|| {
        failure(
            "LZ-DESKTOP-RUNTIME",
            "The voice overlay window is not available.",
        )
    })?;
    let detail = detail.map(|text| text.chars().take(200).collect::<String>());
    let payload = json!({ "state": state, "detail": detail, "level": level });
    if state == OverlayState::Hidden {
        if runtime.overlay_visible.swap(false, Ordering::SeqCst) {
            let _ = app.emit_to(shell::OVERLAY, "voice://overlay", payload);
            let _ = window.hide();
        }
        return Ok(());
    }
    if !runtime.overlay_visible.swap(true, Ordering::SeqCst) {
        shell::place(&app, &window);
        let _ = window.show();
    }
    let _ = app.emit_to(shell::OVERLAY, "voice://overlay", payload);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a WAV with the given format and `samples` 16-bit samples.
    fn wav(format: u16, channels: u16, rate: u32, bits: u16, samples: usize) -> Vec<u8> {
        let data_len = u32::try_from(samples * 2).expect("small");
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&format.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
        out.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.resize(out.len() + samples * 2, 0);
        out
    }

    #[test]
    fn valid_wav_reports_its_duration() {
        assert_eq!(
            validate_wav(&wav(1, 1, 16_000, 16, 16_000)).expect("one second"),
            1000
        );
        assert_eq!(
            validate_wav(&wav(1, 1, 16_000, 16, 8_000)).expect("half"),
            500
        );
        assert_eq!(
            validate_wav(&wav(1, 1, 16_000, 16, 16_000 * 300)).expect("five minutes"),
            300_000
        );
    }

    #[test]
    fn invalid_wavs_fail_with_the_audio_code() {
        let mut list_first = wav(1, 1, 16_000, 16, 160);
        // Insert an odd-sized LIST chunk (with its pad byte) before fmt: must still be accepted.
        let mut with_list = list_first[..12].to_vec();
        with_list.extend_from_slice(b"LIST\x03\x00\x00\x00abc\x00");
        with_list.extend_from_slice(&list_first[12..]);
        assert_eq!(validate_wav(&with_list).expect("extra chunk"), 10);
        list_first.truncate(11);

        let mut truncated_header = wav(1, 1, 16_000, 16, 160);
        truncated_header.truncate(30);
        let mut no_data = wav(1, 1, 16_000, 16, 0);
        no_data.truncate(36);
        let mut not_wave = wav(1, 1, 16_000, 16, 160);
        not_wave[8..12].copy_from_slice(b"AVI ");
        let bad: Vec<Vec<u8>> = vec![
            Vec::new(),
            list_first,
            b"not a wav file at all".to_vec(),
            not_wave,
            truncated_header,
            wav(3, 1, 16_000, 32, 100),
            wav(1, 2, 16_000, 16, 100),
            wav(1, 1, 44_100, 16, 100),
            wav(1, 1, 16_000, 8, 100),
            wav(1, 1, 16_000, 16, 0),
            no_data,
            wav(1, 1, 16_000, 16, 16_000 * 300 + 1),
        ];
        for (index, bytes) in bad.iter().enumerate() {
            let error = validate_wav(bytes).expect_err("invalid");
            assert_eq!(error.code, "LZ-VOICE-AUDIO", "case {index}");
        }
    }

    #[test]
    fn install_guard_enforces_one_install_at_a_time() {
        let runtime = VoiceRuntime::default();
        let first = runtime.begin_install().expect("first");
        assert_eq!(
            runtime.begin_install().err().expect("busy").code,
            "LZ-VOICE-BUSY"
        );
        drop(first);
        assert!(runtime.begin_install().is_ok());
    }

    #[tokio::test]
    async fn cleaning_falls_back_to_basic_when_model_cleaning_is_refused() {
        let voice = std::env::temp_dir().join("lazoo-voice-clean-test-unused");
        let mut settings = VoiceSettings {
            cleaning: Cleaning::Model,
            ..VoiceSettings::default()
        };
        let mut external = false;

        // External endpoint with allowExternal off: refused before any network I/O.
        let (text, used) = run_cleaning(&voice, &settings, "um hello there", &mut external).await;
        assert_eq!(
            (text.as_str(), used, external),
            ("Hello there.", Cleaning::Basic, false)
        );

        settings.cleaning = Cleaning::Basic;
        let (text, used) = run_cleaning(&voice, &settings, "uh, the the end", &mut external).await;
        assert_eq!((text.as_str(), used), ("The end.", Cleaning::Basic));
        settings.cleaning = Cleaning::Off;
        let (text, used) = run_cleaning(&voice, &settings, "uh, the the end", &mut external).await;
        assert_eq!((text.as_str(), used), ("uh, the the end", Cleaning::Off));
        assert_eq!(json!(OverlayState::Transcribing), "transcribing");
    }

    /// Real end-to-end check: downloads the pinned engine and `tiny.en` into a temporary data
    /// root, synthesises speech with Windows SAPI and transcribes it. Run with
    /// `cargo test -p lazoo-desktop -- --ignored live_local_engine --nocapture`.
    #[tokio::test]
    #[ignore = "downloads about 87 MB and runs a real engine"]
    async fn live_local_engine_transcribes_synthesised_speech() {
        let root = std::env::temp_dir().join(format!("lazoo-voice-live-{}", uuid::Uuid::new_v4()));
        // SAFETY: this ignored test is run on its own and owns LAZOO_DATA_ROOT for its duration.
        unsafe { std::env::set_var("LAZOO_DATA_ROOT", &root) };
        let voice = voice_dir().expect("voice dir");

        let stages = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = stages.clone();
        let channel = Channel::<Progress>::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                let json: Value = serde_json::from_str(&json).unwrap_or_default();
                let stage = json["stage"].as_str().unwrap_or_default().to_owned();
                let mut stages = seen.lock().expect("stages");
                if stages.last() != Some(&stage) {
                    stages.push(stage);
                }
            }
            Ok(())
        });
        whisper::install(&voice, WhisperModel::TinyEn, &channel)
            .await
            .expect("install");
        println!("install stages: {:?}", stages.lock().expect("stages"));
        assert!(whisper::binary_path(&voice).is_file());

        let settings = VoiceSettings {
            stt: settings::Stt {
                engine: SttEngine::WhisperLocal,
                model: WhisperModel::TinyEn,
                language: "en".into(),
            },
            hotkey: String::new(),
            ..VoiceSettings::default()
        };
        settings::store(0, settings::validate(settings).expect("valid")).expect("store");

        let wav_path = root.join("speech.wav");
        let script = format!(
            "Add-Type -AssemblyName System.Speech;              $s = New-Object System.Speech.Synthesis.SpeechSynthesizer;              $f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono);              $s.SetOutputToWaveFile('{}', $f);              $s.Speak('Hello world. This is a test of the voice dictation feature.');              $s.Dispose()",
            wav_path.display()
        );
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .expect("powershell");
        assert!(status.success());
        let wav = std::fs::read(&wav_path).expect("speech wav");

        let result = transcribe_wav(&wav).await.expect("transcribe");
        println!(
            "audio {} ms, elapsed {} ms, raw {:?}, text {:?}",
            result.audio_ms, result.elapsed_ms, result.raw, result.text
        );
        let lower = result.text.to_lowercase();
        assert!(lower.contains("hello world"), "{lower}");
        assert!(lower.contains("test"), "{lower}");
        assert_eq!(result.cleaning, Cleaning::Basic);
        assert!(!result.external);
        assert_eq!(
            std::fs::read_dir(voice.join("tmp")).expect("tmp").count(),
            0,
            "temp wav deleted"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
