# Lazoo Voice, keeper autostart and installer verification, 2026-11-04

Branch `feat/voice-mode` (from `main` 718eb8e). Design and contract:
`docs/superpowers/specs/2026-11-04-voice-mode-design.md`. Built by Claude with two Sonnet agents
(desktop Rust side, React side) against that contract; integration, the keeper autostart, the
microphone-block check, the icon and the installer by Claude.

## Automated checks

| Check | Result |
| --- | --- |
| `cargo clippy -p lazoo-desktop --all-targets -- -D warnings` | PASS |
| `cargo test -p lazoo-desktop` | PASS: 51 unit, 7 config; 2 ignored (`live_local_engine`, which downloads and runs the real engine, and the clipboard round trip, which overwrites the real clipboard) |
| `npm --prefix apps/lazoo-desktop/ui run check` | PASS: prettier, typecheck, 121 tests in 10 files, build |

Unit coverage: WAV validation (RIFF/PCM16/mono/16 kHz, empty, over 5 minutes), loopback
classification for the external-processing policy, the basic cleaning table, settings validation
and revision conflicts, hotkey parsing (`Ctrl+Alt+Space`, `Ctrl+Shift+F8`, `Alt+Super+Up`,
`Ctrl+Alt+Backquote`), SHA-256 verification, whisper stdout parsing, temp WAV deletion on failure,
own-origin microphone permission. Front end: WAV header byte for byte, the incremental sentence
chunker (numbers, URLs, code spans, fences, the cap, streaming), speech queue order and fetch-ahead,
composer microphone flow, push-to-talk delivery to all three targets with the refused-insert
fallback, overlay states, settings save and revision reload, key field clearing, install progress,
hotkey capture, read-aloud skipping history and worker turns.

## Live checks by the Rust agent (debug build, temporary data root)

- Local engine end to end through the app's own install code: whisper.cpp b5454 zip and `tiny.en`
  matched their pinned sizes and SHA-256; stages binary, verify, model, verify, done; a 5.56 s
  Windows SAPI WAV transcribed to "Hello world. This is a test of the voice dictation feature." in
  1.18 s; the temporary WAV was gone afterwards. Zip layout: `Release/whisper-cli.exe` with its
  DLLs; flags `-m -f -l -t -nt -np` confirmed from `--help`.
- Endpoint paths against a loopback fake server: multipart transcription (`model`, `language`,
  `file` as `audio.wav`), model cleaning, speech; no `Authorization` header without a key; plain
  `http` accepted only for loopback; redirects refused.
- Hotkey chords produced one `voice://ptt` pressed and one released; `voice_insert_text` typed
  Unicode and a newline into another application and refused with `LZ-VOICE-INSERT-REFUSED` while
  Lazoo was focused; `voice_copy_text` round-tripped Unicode including an emoji.
- Overlay hidden at startup, positioned bottom-centre 64 px above the work area, `WS_EX_NOACTIVATE`
  and topmost; showing and hiding it left the other application focused.
- `voice_speak` bytes arrive in JavaScript as an `ArrayBuffer`.
- Microphone permission: wry 0.55 handles only clipboard permission requests, so the main window
  now allows microphone requests from the app's own origin only. `getUserMedia` then failed with
  `NotReadableError`: Windows privacy settings on this machine deny the microphone for the user
  (`HKCU\...\ConsentStore\microphone` `Value = Deny`, machine-wide `Allow`). Lazoo does not change
  privacy settings; `voice_state.microphoneBlocked` reports it and Settings > Voice opens the page.

## Live run in the real window (Claude)

Debug build of the merged tree (voice plus skills and plugins), a fresh data root, a disposable
home (`USERPROFILE`/`HOME` redirected so no real harness directory was touched), driven over CDP.

- **Keeper autostart.** Only `lazoo-desktop.exe` was started. It spawned `lazoo-keeper.exe`
  (parent: the desktop), which spawned `lazood.exe`; the UI reported Connected with no manual
  step. A second launch against the same data root leaves the running keeper alone.
- **Settings > Voice** rendered the "Windows is blocking the microphone" note with "Open Windows
  settings" (this machine's HKCU microphone consent is Deny) and "Registered" for the hotkey.
- **Model install from the UI.** Install on Tiny, English downloaded whisper.cpp b5454 and
  `ggml-tiny.en.bin` (77,704,715 bytes, the pinned size and SHA-256) into the data root; 15 DLLs
  plus `whisper-cli.exe` under `voice/whisper/b5454/Release`.
- **Transcription through the app** (`voice_transcribe` in the main window, Windows SAPI speech
  because the microphone is blocked): 5.6 s of "Um, please add a retry budget to the dispatcher,
  and uh run the tests." became raw "Um, please add a retry budget to the dispatcher and I run
  the tests." and cleaned "Please add a retry budget to the dispatcher and I run the tests." in
  1.0 to 1.1 s, `external: false`; `voice/tmp` empty afterwards.
- **Overlay** in its real 280 x 56 window: Recording with the level track, then the Inserted
  flash, then hidden; it never took focus.
- **Global hotkey** held for 1.2 s through `keybd_event` while another app was focused: the
  overlay went Recording, then Transcribing.

### Defect found and fixed in the live run

With no speech, whisper returned "you" (its well-known silence hallucination), so push-to-talk
could insert "You." into the focused application; Windows can also hand a denied desktop app
silent buffers. `voice_transcribe` now returns an empty transcript without running any engine
when the loudest sample stays under about -34 dBFS (unit test `silence_and_near_silence_are_gated_but_speech_is_not`),
and the overlay and composer explain an empty result as a microphone block when `voice_state`
reports one. Rechecked live: 1.2 s of silence returned `{"text":"","elapsedMs":0}`; speech still
transcribed in 1.0 s. Note for the owner: the first hotkey test ran before this fix and may have
typed "You." into whichever window had focus at the time.

## Not verified

Real microphone capture on this machine (blocked by the user's Windows privacy setting), any real
external endpoint (none was called), text insertion into elevated windows (Windows blocks it).
