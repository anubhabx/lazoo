# Lazoo Voice, design and desktop contract (2026-11-04)

Implements project brief section 18 (Voice mode) and Stage 8. Owner request: "The voice mode is to
be implemented as well, with transcription cleaning (optional)."

## 1. Scope and placement

Voice is a desktop feature. It lives in `apps/lazoo-desktop` (Rust commands in `src/voice/`, React in
`ui/src`), not in the daemon, so it needs no migration, no daemon RPC and no new lazooctl verb. Speech
reaches an employee the same way typed input does: the transcript becomes the text of an ordinary
`turn.submit`, so the durable conversation turn stays authoritative (section 18: "Speech is
transcribed into the same durable conversation turn as typed input").

Two surfaces (section 18):

1. **Integrated mode**: a microphone control in the conversation composer, and an optional "read
   replies aloud" mode that speaks the lead's reply as it streams, sentence by sentence.
2. **Standalone dictation**: a configurable global push-to-talk hotkey and a small always-on-top
   overlay. Hold the hotkey anywhere in Windows, speak, release: the cleaned text is inserted into
   the focused application (or copied, or sent to the open conversation's lead, per setting).

## 2. Engines

| Role | Engine | Where it runs | External? |
|---|---|---|---|
| Speech to text | `whisper-local`: whisper.cpp CLI, pinned build, ggml model | Child process of the desktop, below-normal priority, bounded threads | No |
| Speech to text | `endpoint`: OpenAI-compatible `POST {baseUrl}/audio/transcriptions` | HTTP from the desktop's Rust side | Yes unless `baseUrl` host is loopback |
| Text to speech | `system`: Windows voices through WebView2 `speechSynthesis` | Front end | No |
| Text to speech | `endpoint`: OpenAI-compatible `POST {baseUrl}/audio/speech` (mp3) | HTTP from Rust, bytes played in the front end | Yes unless loopback |
| Cleaning | `basic`: deterministic rules (fillers, stutters, spacing, capitalisation, final punctuation) | Rust | No |
| Cleaning | `model`: OpenAI-compatible `POST {baseUrl}/chat/completions` with a fixed instruction | HTTP from Rust | Yes unless loopback |

One OpenAI-compatible endpoint configuration covers hosted APIs (OpenAI, Groq and similar) and local
servers (for example a loopback faster-whisper or Kokoro server), so "future local options" need no
new engine.

Pinned local engine (download on demand from Settings, verified by SHA-256 before use, stored under
the data root at `voice/whisper/b5454/` and `voice/models/`):

| Asset | URL | Bytes | SHA-256 |
|---|---|---|---|
| whisper.cpp CPU x64 build b5454 | `https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip` | 8928640 | `6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb` |
| `tiny.en` | `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin` | 77704715 | `921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f` |
| `base.en` | `.../ggml-base.en.bin` | 147964211 | `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002` |
| `small.en` | `.../ggml-small.en.bin` | 487614201 | `c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d` |
| `base` (multilingual) | `.../ggml-base.bin` | 147951465 | `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` |
| `small` (multilingual) | `.../ggml-small.bin` | 487601967 | `1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b` |

The zip is extracted with Windows' own `tar.exe` (no new crate). A hash mismatch deletes the partial
file and fails `LZ-VOICE-DOWNLOAD`.

## 3. Privacy and policy (section 18 requirements)

- **Audio is never retained.** Captured audio stays in memory. The local engine needs a file, so the
  WAV is written to an owner-protected `voice/tmp/` under the data root and deleted on every path
  (success, failure, timeout, panic guard); `voice/tmp/` is emptied at startup. No audio or
  transcript text is logged.
- **Nothing leaves the machine without explicit policy.** An engine is external when its endpoint
  host is not loopback (`localhost`, `127.0.0.0/8`, `::1`). Every external call checks
  `allowExternal` first and fails `LZ-VOICE-EXTERNAL-REFUSED` before any network I/O. The default is
  `false`. The UI shows an "External" marker with the host beside every control that would send
  audio or text out.
- **Clear indicators.** Recording shows a live level meter and a "Recording" label in the composer or
  the overlay; transcribing shows "Transcribing". The overlay never takes focus.
- **No spoken permission.** Voice only produces text. It never answers an approval card, never
  presses an Inbox action and never widens authority.
- **Secrets.** The endpoint API key is submitted once, protected with DPAPI for the current user
  (`lazoo_runtime::protect_for_current_user`) in `voice/endpoint.key`, never returned to the front
  end, never logged, never placed in a URL or process argument.
- **Resource budget.** The local engine runs only while transcribing, at below-normal priority with
  `max(2, logical processors / 4)` threads, capped by a 120 s deadline.

## 4. Settings

Stored by the desktop at `<data root>/voice/settings.json` (atomic write, owner-protected directory),
revision-checked.

```ts
type SttEngine = "off" | "whisper-local" | "endpoint";
type TtsEngine = "off" | "system" | "endpoint";
type Cleaning = "off" | "basic" | "model";
type DictationTarget = "insert" | "clipboard" | "conversation";
type WhisperModel = "tiny.en" | "base.en" | "small.en" | "base" | "small";

interface VoiceSettings {
  stt: { engine: SttEngine; model: WhisperModel; language: string }; // "auto" or ISO 639-1
  tts: { engine: TtsEngine; voice: string; rate: number };          // rate 0.5..2.0; voice "" = default
  endpoint: {
    baseUrl: string;            // https://api.openai.com/v1
    transcriptionModel: string; // gpt-4o-mini-transcribe
    speechModel: string;        // gpt-4o-mini-tts
    speechVoice: string;        // alloy
    cleanupModel: string;       // gpt-4o-mini
  };
  cleaning: Cleaning;           // "basic"
  allowExternal: boolean;       // false
  reviewBeforeSend: boolean;    // true: transcript lands in the composer for correction
  readReplies: boolean;         // false
  hotkey: string;               // "Ctrl+Alt+Space"; "" disables the global hotkey
  dictationTarget: DictationTarget; // "insert"
}

interface VoiceState {
  revision: number;
  settings: VoiceSettings;
  keySet: boolean;
  endpointExternal: boolean;
  whisper: {
    binaryInstalled: boolean;
    installedModels: WhisperModel[];
    catalog: { model: WhisperModel; bytes: number; installed: boolean }[];
  };
  hotkeyRegistered: boolean;
  hotkeyError: string | null;
}
```

Defaults are the values in the comments; `stt.engine` defaults to `whisper-local` with `base.en` and
language `en`; `tts.engine` to `system`.

## 5. Desktop commands (Tauri)

All failures are `{ code, message }` like `DesktopFailure`.

| Command | Arguments | Result | Notes |
|---|---|---|---|
| `voice_state` | none | `VoiceState` | |
| `voice_settings_set` | `{ settings: VoiceSettings, revision: number }` | `VoiceState` | `LZ-VOICE-REVISION` on a stale revision, `LZ-VOICE-SETTINGS` on invalid values (bad URL, rate out of range, unknown model, unparsable hotkey). Re-registers the hotkey. |
| `voice_key_set` | `{ key: string }` | `VoiceState` | DPAPI-protected; empty key refused |
| `voice_key_clear` | none | `VoiceState` | |
| `voice_whisper_install` | `{ model: WhisperModel, onProgress: Channel<{ stage: "binary" \| "model" \| "verify" \| "done", received: number, total: number }> }` | `VoiceState` | Installs the binary if missing, then the model. Resumable by re-running; one install at a time (`LZ-VOICE-BUSY`). |
| `voice_whisper_remove` | `{ model: WhisperModel \| null }` | `VoiceState` | `null` removes the binary and every model |
| `voice_transcribe` | raw request body: WAV bytes (RIFF, PCM 16-bit, mono, 16 kHz); header `x-lazoo-purpose: conversation \| dictation` | `{ text, raw, cleaning: Cleaning, engine: SttEngine, external: boolean, audioMs, elapsedMs }` | `LZ-VOICE-OFF`, `LZ-VOICE-AUDIO` (not that WAV format, empty, or longer than 5 minutes), `LZ-VOICE-ENGINE-MISSING`, `LZ-VOICE-EXTERNAL-REFUSED`, `LZ-VOICE-KEY-MISSING`, `LZ-VOICE-ENDPOINT` (status and a redacted, bounded body excerpt), `LZ-VOICE-TIMEOUT`. `text` is `raw` after cleaning; a failed model cleaning falls back to basic and says so in `cleaning`. |
| `voice_speak` | `{ text: string }` | raw `audio/mpeg` bytes (`tauri::ipc::Response`) | Endpoint TTS only; `system` TTS never calls Rust. Text capped at 4096 chars. |
| `voice_insert_text` | `{ text: string }` | none | `SendInput` with `KEYEVENTF_UNICODE` into the foreground window; `\n` becomes Enter. `LZ-VOICE-INSERT-REFUSED` when the foreground window belongs to Lazoo (the front end then fills its own composer) or no window has focus. |
| `voice_overlay` | `{ state: "hidden" \| "recording" \| "transcribing" \| "done" \| "error", detail: string \| null, level: number \| null }` | none | Shows or hides the overlay window and forwards the state to it. |

Events from Rust:

- `voice://ptt` `{ state: "pressed" | "released" }`, emitted to the `main` window when the global
  hotkey is pressed or released.
- `voice://overlay` `{ state, detail, level }`, emitted to the `voice-overlay` window.

Windows: `main` (unchanged) and `voice-overlay`, created hidden at startup: 280 x 56 logical px,
no decorations, transparent, always on top, skip taskbar, not focusable, not resizable, no shadow,
bottom-centre of the primary monitor's work area 64 px above its bottom edge. It loads the same
front end; the front end renders only the overlay when its window label is `voice-overlay`.
Recording always happens in the `main` window's web view (it stays alive while minimised).

CSP gains `media-src 'self' blob:` so endpoint speech can play from a blob URL.

## 6. Front end behaviour

- **Capture**: `getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } })`,
  an `AudioContext` at 16 kHz with an `AudioWorklet` (or `ScriptProcessorNode` fallback) collecting
  Float32 frames, encoded to PCM16 WAV on stop. The stream's tracks stop as soon as recording ends.
  A level (RMS) drives the meter. Under 300 ms of audio is discarded silently.
- **Composer**: a microphone button beside Send (click to start, click or Enter to stop, Esc to
  cancel; `Ctrl+Shift+Space` toggles while Lazoo is focused). With `reviewBeforeSend` the transcript
  is inserted at the caret for correction; otherwise it is sent as the turn. An "External · host"
  marker appears when the active STT engine is external.
- **Read replies aloud**: when `readReplies` is on and a conversation is open, the lead's
  `assistantText` is spoken as it grows: everything up to the last sentence boundary is queued
  (chunks of at most about 240 characters, split at `.`, `!`, `?`, `:` or newline, never inside a
  number like `3.5` or a code span), the remainder is flushed when the turn completes. `system` uses
  `speechSynthesis` utterances; `endpoint` fetches audio for at most two chunks ahead (backpressure)
  and plays them in order. Controls in the agent bar: speaking indicator, Stop, Replay last reply.
  Esc stops speech. A TTS failure stops the queue, shows a toast, and leaves the text as is
  (text-only fallback).
- **Global push-to-talk**: on `voice://ptt` pressed, start recording and show the overlay
  ("Recording", live level); on released, stop, show "Transcribing", transcribe with purpose
  `dictation`, then deliver per `dictationTarget`: `insert` calls `voice_insert_text` (on
  `LZ-VOICE-INSERT-REFUSED` with a conversation open, put the text in its composer instead),
  `clipboard` writes the clipboard, `conversation` submits to the open conversation's lead (or falls
  back to clipboard with a toast when none is open). The overlay flashes "Inserted", "Copied" or
  "Sent" for 900 ms, or shows the error for 3 s.
- **Settings → Voice** page: speech to text (engine, model install/remove with size and progress,
  language), cleaning, text to speech (system voice list from `speechSynthesis.getVoices()`, rate,
  preview button), endpoint (base URL, models, voice, key set/rotate/clear; password field clears
  after submit), privacy (allow external processing, plain statement that audio is never stored),
  push-to-talk (hotkey capture, dictation target, review before send).

## 7. Deferred

Streaming partial transcription while still speaking, voice activity detection (hands-free), a
tray-resident overlay when the main window is closed, wake words, per-conversation voice selection,
GPU builds of whisper.cpp.
