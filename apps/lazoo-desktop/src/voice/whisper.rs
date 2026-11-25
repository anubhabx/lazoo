//! The pinned local engine: whisper.cpp CLI plus ggml models, downloaded on demand and verified by
//! SHA-256, then run as a below-normal-priority child on a temporary WAV.
//!
//! Layout of `whisper-bin-x64.zip` (b5454, observed by extracting it): a single top-level
//! `Release/` directory holding `whisper-cli.exe` next to its DLLs (`whisper.dll`, `ggml.dll`,
//! `ggml-base.dll`, `ggml-cpu-*.dll`, `llama.dll`, `parakeet.dll`, `SDL2.dll`) and ~30 other tools
//! (`whisper-server.exe`, `whisper-bench.exe`, ...). Only `Release/whisper-cli.exe` is used; the
//! DLLs are found because they sit in the executable's own directory.
//!
//! Verified `whisper-cli.exe --help` flags: `-m FNAME` model, `-f FNAME` input, `-l LANG`
//! ("auto" detects), `-t N` threads, `-nt` no timestamps, `-np` print only results.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::ipc::Channel;
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};
use uuid::Uuid;

use super::{failure, settings::ensure_dir};
use crate::client::DesktopFailure;

const RELEASE: &str = "b5454";
const BINARY_URL: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip";
const BINARY_BYTES: u64 = 8_928_640;
const BINARY_SHA256: &str = "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb";
const MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";
pub const TRANSCRIBE_DEADLINE: Duration = Duration::from_secs(120);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WhisperModel {
    #[serde(rename = "tiny.en")]
    TinyEn,
    #[serde(rename = "base.en")]
    BaseEn,
    #[serde(rename = "small.en")]
    SmallEn,
    #[serde(rename = "base")]
    Base,
    #[serde(rename = "small")]
    Small,
}

impl WhisperModel {
    pub const ALL: [Self; 5] = [
        Self::TinyEn,
        Self::BaseEn,
        Self::SmallEn,
        Self::Base,
        Self::Small,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|model| model.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::TinyEn => "tiny.en",
            Self::BaseEn => "base.en",
            Self::SmallEn => "small.en",
            Self::Base => "base",
            Self::Small => "small",
        }
    }

    pub const fn is_english_only(self) -> bool {
        matches!(self, Self::TinyEn | Self::BaseEn | Self::SmallEn)
    }

    pub const fn bytes(self) -> u64 {
        match self {
            Self::TinyEn => 77_704_715,
            Self::BaseEn => 147_964_211,
            Self::SmallEn => 487_614_201,
            Self::Base => 147_951_465,
            Self::Small => 487_601_967,
        }
    }

    const fn sha256(self) -> &'static str {
        match self {
            Self::TinyEn => "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
            Self::BaseEn => "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
            Self::SmallEn => "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
            Self::Base => "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            Self::Small => "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        }
    }

    fn file_name(self) -> String {
        format!("ggml-{}.bin", self.name())
    }
}

pub fn binary_dir(voice: &Path) -> PathBuf {
    voice.join("whisper").join(RELEASE)
}

pub fn binary_path(voice: &Path) -> PathBuf {
    binary_dir(voice).join("Release").join("whisper-cli.exe")
}

pub fn models_dir(voice: &Path) -> PathBuf {
    voice.join("models")
}

pub fn model_path(voice: &Path, model: WhisperModel) -> PathBuf {
    models_dir(voice).join(model.file_name())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    model: WhisperModel,
    bytes: u64,
    installed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhisperState {
    binary_installed: bool,
    installed_models: Vec<WhisperModel>,
    catalog: Vec<CatalogEntry>,
}

pub fn state(voice: &Path) -> WhisperState {
    // A model counts as installed only at its pinned size: partial downloads live in `.part` files.
    let installed = |model: WhisperModel| {
        fs::metadata(model_path(voice, model)).is_ok_and(|meta| meta.len() == model.bytes())
    };
    WhisperState {
        binary_installed: binary_path(voice).is_file(),
        installed_models: WhisperModel::ALL
            .into_iter()
            .filter(|model| installed(*model))
            .collect(),
        catalog: WhisperModel::ALL
            .into_iter()
            .map(|model| CatalogEntry {
                model,
                bytes: model.bytes(),
                installed: installed(model),
            })
            .collect(),
    }
}

pub fn remove(voice: &Path, model: Option<WhisperModel>) {
    if let Some(model) = model {
        let _ = fs::remove_file(model_path(voice, model));
        let _ = fs::remove_file(models_dir(voice).join(format!("{}.part", model.file_name())));
    } else {
        let _ = fs::remove_dir_all(voice.join("whisper"));
        let _ = fs::remove_dir_all(models_dir(voice));
    }
}

// ---------------------------------------------------------------- download and install

#[derive(Clone, Serialize)]
pub struct Progress {
    pub stage: &'static str,
    pub received: u64,
    pub total: u64,
}

fn download_failure(message: &str) -> DesktopFailure {
    failure("LZ-VOICE-DOWNLOAD", message)
}

pub fn hash_file_into(path: &Path, hasher: &mut Sha256) -> std::io::Result<u64> {
    let mut file = fs::File::open(path)?;
    let mut buffer = vec![0_u8; 1 << 20];
    let mut total = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(total);
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
}

pub fn hex(digest: &[u8]) -> String {
    use std::fmt::Write;
    digest.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// True when `path` has exactly the pinned size and SHA-256.
pub fn file_matches(path: &Path, bytes: u64, sha256: &str) -> bool {
    let mut hasher = Sha256::new();
    hash_file_into(path, &mut hasher).is_ok_and(|size| size == bytes)
        && hex(&hasher.finalize()) == sha256
}

/// Hash and length of an existing partial download, or a fresh start when it is unusable.
async fn resume_point(part: &Path, bytes: u64) -> (Sha256, u64) {
    let path = part.to_path_buf();
    let seeded = tokio::task::spawn_blocking(move || {
        let mut digest = Sha256::new();
        hash_file_into(&path, &mut digest).map(|size| (digest, size))
    })
    .await;
    match seeded {
        Ok(Ok((digest, size))) if size <= bytes => (digest, size),
        _ => {
            let _ = fs::remove_file(part);
            (Sha256::new(), 0)
        }
    }
}

/// Streams `url` into `part` (resuming a partial file with a Range request), hashing as it goes.
/// On success `part` is exactly the pinned asset; a hash mismatch deletes it.
#[allow(clippy::too_many_lines)] // One linear resume, stream, verify sequence.
async fn fetch(
    url: &str,
    bytes: u64,
    sha256: &str,
    part: &Path,
    stage: &'static str,
    on_progress: &Channel<Progress>,
) -> Result<(), DesktopFailure> {
    let (mut digest, mut received) = if part.exists() {
        resume_point(part, bytes).await
    } else {
        (Sha256::new(), 0)
    };
    let progress = |stage: &'static str, received: u64| {
        let _ = on_progress.send(Progress {
            stage,
            received,
            total: bytes,
        });
    };
    progress(stage, received);

    if received < bytes {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .user_agent("lazoo-desktop")
            .build()
            .map_err(|_| download_failure("The download client could not start."))?;
        let mut request = client.get(url);
        if received > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={received}-"));
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| download_failure("The download server could not be reached."))?;
        let status = response.status();
        if received > 0 && status == reqwest::StatusCode::OK {
            // The server ignored the Range header: start over from byte zero.
            digest = Sha256::new();
            received = 0;
        } else if received > 0 && status != reqwest::StatusCode::PARTIAL_CONTENT {
            let _ = fs::remove_file(part);
            return Err(download_failure(
                "The partial download could not be resumed and was discarded. Try again.",
            ));
        } else if !status.is_success() {
            return Err(download_failure(&format!(
                "The download server answered HTTP {}.",
                status.as_u16()
            )));
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(received > 0)
            .truncate(received == 0)
            .open(part)
            .await
            .map_err(|_| download_failure("The download file could not be written."))?;
        let mut last_report = Instant::now();
        loop {
            let chunk = timeout(Duration::from_secs(60), response.chunk())
                .await
                .map_err(|_| download_failure("The download stalled."))?
                .map_err(|_| {
                    download_failure("The download was interrupted. Try again to resume.")
                })?;
            let Some(chunk) = chunk else { break };
            received += chunk.len() as u64;
            if received > bytes {
                drop(file);
                let _ = fs::remove_file(part);
                return Err(download_failure(
                    "The download was larger than the pinned asset.",
                ));
            }
            digest.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| download_failure("The download file could not be written."))?;
            if last_report.elapsed() >= Duration::from_millis(100) {
                last_report = Instant::now();
                progress(stage, received);
            }
        }
        file.flush()
            .await
            .map_err(|_| download_failure("The download file could not be written."))?;
    }

    if received < bytes {
        return Err(download_failure(
            "The download ended early. Try again to resume.",
        ));
    }
    progress("verify", bytes);
    if hex(&digest.finalize()) != sha256 {
        let _ = fs::remove_file(part);
        return Err(download_failure(
            "The download failed SHA-256 verification and was deleted.",
        ));
    }
    Ok(())
}

async fn install_binary(
    voice: &Path,
    on_progress: &Channel<Progress>,
) -> Result<(), DesktopFailure> {
    let parent = ensure_dir(&voice.join("whisper"))?;
    let zip = parent.join(format!("{RELEASE}.zip.part"));
    fetch(
        BINARY_URL,
        BINARY_BYTES,
        BINARY_SHA256,
        &zip,
        "binary",
        on_progress,
    )
    .await?;

    let staging = parent.join(format!(".{RELEASE}.{}.extract", Uuid::new_v4()));
    let result = extract(&zip, &staging, &binary_dir(voice)).await;
    let _ = fs::remove_dir_all(&staging);
    if result.is_ok() {
        let _ = fs::remove_file(&zip);
    }
    result
}

/// Extracts with Windows' own tar.exe into `staging`, checks for the CLI, then swaps into place.
async fn extract(zip: &Path, staging: &Path, destination: &Path) -> Result<(), DesktopFailure> {
    let failed = || download_failure("The whisper.cpp archive could not be extracted.");
    fs::create_dir_all(staging).map_err(|_| failed())?;
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let tar = PathBuf::from(system_root).join("System32").join("tar.exe");
    let status = timeout(
        Duration::from_secs(120),
        Command::new(tar)
            .arg("-xf")
            .arg(zip)
            .arg("-C")
            .arg(staging)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .map_err(|_| failed())?
    .map_err(|_| failed())?;
    if !status.success() || !staging.join("Release").join("whisper-cli.exe").is_file() {
        return Err(failed());
    }
    let _ = fs::remove_dir_all(destination);
    fs::rename(staging, destination).map_err(|_| failed())
}

pub async fn install(
    voice: &Path,
    model: WhisperModel,
    on_progress: &Channel<Progress>,
) -> Result<(), DesktopFailure> {
    if !binary_path(voice).is_file() {
        install_binary(voice, on_progress).await?;
    }
    let target = model_path(voice, model);
    if !file_matches(&target, model.bytes(), model.sha256()) {
        let directory = ensure_dir(&models_dir(voice))?;
        let part = directory.join(format!("{}.part", model.file_name()));
        let url = format!("{MODEL_BASE_URL}{}", model.file_name());
        fetch(
            &url,
            model.bytes(),
            model.sha256(),
            &part,
            "model",
            on_progress,
        )
        .await?;
        fs::rename(&part, &target)
            .map_err(|_| download_failure("The verified model could not be moved into place."))?;
    }
    let _ = on_progress.send(Progress {
        stage: "done",
        received: model.bytes(),
        total: model.bytes(),
    });
    Ok(())
}

// ---------------------------------------------------------------- transcription

/// Owner-protected WAV under `voice/tmp/` that is deleted when dropped, whatever the path.
pub struct TempWav(PathBuf);

impl TempWav {
    pub fn create(directory: &Path, wav: &[u8]) -> Result<Self, DesktopFailure> {
        use std::io::Write;
        let failed = || {
            failure(
                "LZ-DESKTOP-RUNTIME",
                "The temporary audio file could not be created.",
            )
        };
        let path = directory.join(format!("{}.wav", Uuid::new_v4()));
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|_| failed())?;
        let guard = Self(path);
        lazoo_runtime::OwnerSecurity::current()
            .and_then(|owner| owner.apply_to_path(&guard.0))
            .map_err(|_| failed())?;
        file.write_all(wav).map_err(|_| failed())?;
        Ok(guard)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        // A killed child may hold the file for a moment after a timeout, so retry briefly.
        for _ in 0..20 {
            if fs::remove_file(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Deletes anything left in `voice/tmp/` by an earlier run that died mid-transcription.
pub fn sweep_tmp(voice: &Path) {
    if let Ok(entries) = fs::read_dir(voice.join("tmp")) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn threads() -> usize {
    let logical = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
    (logical / 4).max(2)
}

/// Runs whisper-cli on `wav` and returns the transcript text.
pub async fn transcribe(
    voice: &Path,
    model: WhisperModel,
    language: &str,
    wav: &[u8],
) -> Result<String, DesktopFailure> {
    let binary = binary_path(voice);
    let model_file = model_path(voice, model);
    if !binary.is_file() || !model_file.is_file() {
        return Err(failure(
            "LZ-VOICE-ENGINE-MISSING",
            "The local speech engine or model is not installed. Install it in Settings, Voice.",
        ));
    }
    let language = if model.is_english_only() {
        "en"
    } else {
        language
    };
    let tmp = ensure_dir(&voice.join("tmp"))?;
    transcribe_with(
        &binary,
        &model_file,
        &tmp,
        language,
        wav,
        TRANSCRIBE_DEADLINE,
    )
    .await
}

async fn transcribe_with(
    binary: &Path,
    model_file: &Path,
    tmp: &Path,
    language: &str,
    wav: &[u8],
    deadline: Duration,
) -> Result<String, DesktopFailure> {
    let audio = TempWav::create(tmp, wav)?;
    let mut command = Command::new(binary);
    command
        .arg("-m")
        .arg(model_file)
        .arg("-f")
        .arg(audio.path())
        .args(["-l", language, "-nt", "-np", "-t"])
        .arg(threads().to_string())
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(directory) = binary.parent() {
        command.current_dir(directory);
    }
    let output = timeout(deadline, command.output())
        .await
        .map_err(|_| {
            failure(
                "LZ-VOICE-TIMEOUT",
                "The local speech engine did not finish in time.",
            )
        })?
        .map_err(|_| {
            failure(
                "LZ-VOICE-ENGINE-FAILED",
                "The local speech engine could not start.",
            )
        })?;
    if !output.status.success() {
        return Err(failure(
            "LZ-VOICE-ENGINE-FAILED",
            &format!(
                "The local speech engine failed (exit code {}).",
                output
                    .status
                    .code()
                    .map_or_else(|| "unknown".into(), |code| code.to_string())
            ),
        ));
    }
    Ok(parse_output(&output.stdout))
}

/// With `-nt -np` whisper-cli prints one text segment per line. Non-speech tags such as
/// `[BLANK_AUDIO]` or `(music)` are dropped and the segments are joined with single spaces.
pub fn parse_output(stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(stdout);
    let mut words = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let tag_only = (line.starts_with('[') && line.ends_with(']'))
            || (line.starts_with('(') && line.ends_with(')'));
        if line.is_empty() || tag_only {
            continue;
        }
        words.extend(
            line.replace("[BLANK_AUDIO]", " ")
                .split_whitespace()
                .map(str::to_owned),
        );
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_is_joined_and_non_speech_tags_are_dropped() {
        assert_eq!(
            parse_output(b" Hello there.\r\n How are you?\r\n"),
            "Hello there. How are you?"
        );
        assert_eq!(parse_output(b"[BLANK_AUDIO]\n"), "");
        assert_eq!(
            parse_output(b" (music)\n [ Silence ]\n Okay then.\n"),
            "Okay then."
        );
        assert_eq!(parse_output(b" Yes [BLANK_AUDIO] no.\n"), "Yes no.");
        assert_eq!(parse_output(b" caf\xc3\xa9 au lait\n"), "caf\u{e9} au lait");
        assert_eq!(parse_output(b""), "");
    }

    #[test]
    fn models_parse_by_their_contract_names() {
        for model in WhisperModel::ALL {
            assert_eq!(WhisperModel::parse(model.name()), Some(model));
            assert_eq!(
                serde_json::to_value(model).expect("json"),
                serde_json::Value::String(model.name().into())
            );
        }
        assert_eq!(WhisperModel::parse("huge"), None);
        assert!(WhisperModel::TinyEn.is_english_only() && !WhisperModel::Base.is_english_only());
        assert!(threads() >= 2);
    }

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lazoo-whisper-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn sha256_verification_accepts_the_pinned_digest_and_rejects_others() {
        let dir = scratch();
        let file = dir.join("fixture.bin");
        fs::write(&file, b"abc").expect("fixture");
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(file_matches(&file, 3, abc));
        assert!(!file_matches(&file, 4, abc), "wrong size");
        assert!(!file_matches(&file, 3, &"0".repeat(64)), "wrong digest");
        assert!(!file_matches(&dir.join("missing"), 3, abc));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn temp_wav_is_owner_file_deleted_on_drop() {
        let dir = scratch();
        let wav = TempWav::create(&dir, b"RIFF").expect("create");
        let path = wav.path().to_path_buf();
        assert_eq!(fs::read(&path).expect("read"), b"RIFF");
        drop(wav);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sweep_removes_leftovers() {
        let voice = scratch();
        fs::create_dir_all(voice.join("tmp")).expect("tmp");
        fs::write(voice.join("tmp").join("old.wav"), b"x").expect("leftover");
        sweep_tmp(&voice);
        assert_eq!(fs::read_dir(voice.join("tmp")).expect("dir").count(), 0);
        let _ = fs::remove_dir_all(voice);
    }

    #[tokio::test]
    async fn temp_wav_is_deleted_when_the_engine_cannot_start() {
        let dir = scratch();
        let error = transcribe_with(
            &dir.join("no-such-whisper.exe"),
            &dir.join("model.bin"),
            &dir,
            "en",
            b"RIFF",
            Duration::from_secs(5),
        )
        .await
        .expect_err("missing binary");
        assert_eq!(error.code, "LZ-VOICE-ENGINE-FAILED");
        assert_eq!(
            fs::read_dir(&dir).expect("dir").count(),
            0,
            "no wav left behind"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn temp_wav_is_deleted_when_the_engine_exits_nonzero_or_times_out() {
        let dir = scratch();
        let system =
            PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot")).join("System32");

        // ping.exe rejects the whisper-style arguments ("Bad option -m") with a non-zero code.
        let error = transcribe_with(
            &system.join("PING.EXE"),
            &dir.join("model.bin"),
            &dir,
            "en",
            b"RIFF",
            Duration::from_secs(20),
        )
        .await
        .expect_err("non-zero exit");
        assert_eq!(error.code, "LZ-VOICE-ENGINE-FAILED");
        assert_eq!(fs::read_dir(&dir).expect("dir").count(), 0);

        // A zero deadline always elapses before the child can finish.
        let error = transcribe_with(
            &system.join("PING.EXE"),
            &dir.join("model.bin"),
            &dir,
            "en",
            b"RIFF",
            Duration::ZERO,
        )
        .await
        .expect_err("deadline");
        assert_eq!(error.code, "LZ-VOICE-TIMEOUT");
        assert_eq!(fs::read_dir(&dir).expect("dir").count(), 0);
        let _ = fs::remove_dir_all(dir);
    }
}
