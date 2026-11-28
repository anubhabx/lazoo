// Voice in the UI: the composer's microphone, the Voice settings page, the
// dictation overlay, global push-to-talk and read-aloud, against a fake bridge
// and a fake recorder (jsdom has no microphone).

import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { App } from "./App";
import { VoiceOverlay } from "./components/VoiceOverlay";
import { ReadAloud } from "./components/ReadAloud";
import {
  BridgeProvider,
  StoreProvider,
  TerminalsProvider,
} from "./lib/context";
import { Store } from "./lib/store";
import { TerminalManager } from "./lib/terminals";
import type { Recorder } from "./lib/voice/capture";
import { VoiceProvider } from "./lib/voice/context";
import { VoiceController, type VoiceDeps } from "./lib/voice/controller";
import type { SpeechDeps } from "./lib/voice/speech";
import {
  DEFAULT_VOICE_SETTINGS,
  type AudioClip,
  type TranscribeResult,
  type VoiceSettings,
  type VoiceState,
} from "./lib/voice/types";
import { CommandPalette } from "./components/CommandPalette";
import { SettingsPage } from "./pages/Settings";
import { Composer } from "./pages/Conversation";
import { createFakeBridge, type FakeBridge } from "./test/fakeBridge";
import {
  CONVERSATION_ID,
  TASK_ID,
  conversation,
  turnRecord,
  turnSummary,
} from "./test/fixtures";

const CLIP: AudioClip = {
  wav: new Uint8Array([82, 73, 70, 70]),
  durationMs: 1200,
};

function voiceState(
  overrides: Partial<VoiceSettings> = {},
  extra: Partial<VoiceState> = {},
): VoiceState {
  return {
    revision: 3,
    settings: { ...structuredClone(DEFAULT_VOICE_SETTINGS), ...overrides },
    keySet: false,
    endpointExternal: true,
    whisper: {
      binaryInstalled: true,
      installedModels: ["base.en"],
      catalog: [
        { model: "tiny.en", bytes: 77_704_715, installed: false },
        { model: "base.en", bytes: 147_964_211, installed: true },
        { model: "small.en", bytes: 487_614_201, installed: false },
      ],
    },
    hotkeyRegistered: true,
    hotkeyError: null,
    microphoneBlocked: false,
    ...extra,
  };
}

function transcript(text: string): TranscribeResult {
  return {
    text,
    raw: text,
    cleaning: "basic",
    engine: "whisper-local",
    external: false,
    audioMs: 1200,
    elapsedMs: 300,
  };
}

function fakeRecorder(clip: AudioClip | null = CLIP) {
  let onLevel: (level: number) => void = () => {};
  const recorder: Recorder & { level: (value: number) => void } = {
    start: vi.fn(async (cb: (level: number) => void) => {
      onLevel = cb;
    }),
    stop: vi.fn(async () => clip),
    cancel: vi.fn(),
    level: (value) => onLevel(value),
  };
  return recorder;
}

function fakeSpeech() {
  const spoken: string[] = [];
  const deps: SpeechDeps = {
    speakSystem: (text) => {
      spoken.push(text);
      return { done: Promise.resolve(), cancel: () => undefined };
    },
    fetchAudio: async () => new ArrayBuffer(1),
    playAudio: () => ({ done: Promise.resolve(), cancel: () => undefined }),
  };
  return { spoken, deps };
}

function scripted(state: VoiceState | null = voiceState()): FakeBridge {
  const bridge = createFakeBridge();
  if (state) bridge.voice.state.mockResolvedValue(state);
  bridge.on("workspace.conversation", conversation());
  bridge.on("session.list", []);
  bridge.on("turn.submit", { message: { delivery: { state: "delivered" } } });
  return bridge;
}

async function mountVoice(
  bridge: FakeBridge,
  deps: Partial<VoiceDeps> = {},
  ui: (store: Store, voice: VoiceController) => React.ReactNode = () => null,
) {
  const store = new Store(bridge, { statusPollMs: 0, debounceMs: 10 });
  const voice = new VoiceController(bridge, store, deps);
  const terminals = new TerminalManager(bridge);
  const view = render(
    <StoreProvider value={store}>
      <BridgeProvider value={bridge}>
        <TerminalsProvider value={terminals}>
          <VoiceProvider value={voice}>{ui(store, voice)}</VoiceProvider>
        </TerminalsProvider>
      </BridgeProvider>
    </StoreProvider>,
  );
  await act(async () => {
    await voice.start();
  });
  return { store, voice, ...view };
}

async function openConversation(store: Store) {
  await act(async () => {
    store.navigate({ kind: "conversation", id: CONVERSATION_ID, tab: "lead" });
    await store.refresh("conversation");
  });
}

beforeEach(() => {
  localStorage.clear();
});

describe("composer microphone", () => {
  test("start, stop: the transcript is inserted at the caret for review", async () => {
    const bridge = scripted();
    bridge.voice.transcribe.mockResolvedValue(
      transcript("check the retry test"),
    );
    const recorder = fakeRecorder();
    const user = userEvent.setup();
    const { store, voice } = await mountVoice(
      bridge,
      { createRecorder: () => recorder },
      () => <Composer />,
    );
    await openConversation(store);

    const box = screen.getByLabelText("Message to the lead");
    await user.type(box, "please also");
    await user.click(screen.getByRole("button", { name: "Dictate a message" }));
    const status = await screen.findByText("Recording");
    expect(status.closest("[role=status]")).toHaveAttribute(
      "aria-live",
      "polite",
    );
    expect(recorder.start).toHaveBeenCalled();
    expect(voice.getSnapshot().phase).toBe("recording");

    await user.click(screen.getByRole("button", { name: "Stop dictation" }));
    await waitFor(() =>
      expect(box).toHaveValue("please also check the retry test"),
    );
    expect(bridge.voice.transcribe).toHaveBeenCalledWith(
      CLIP.wav,
      "conversation",
    );
    // Review before send: nothing was submitted.
    expect(bridge.callsTo("turn.submit")).toHaveLength(0);
    expect(screen.queryByText("Recording")).not.toBeInTheDocument();
    expect(recorder.cancel).not.toHaveBeenCalled();
  });

  test("with review off the transcript is sent as the turn", async () => {
    const bridge = scripted(voiceState({ reviewBeforeSend: false }));
    bridge.voice.transcribe.mockResolvedValue(
      transcript("run the whole suite"),
    );
    const user = userEvent.setup();
    const { store } = await mountVoice(
      bridge,
      { createRecorder: () => fakeRecorder() },
      () => <Composer />,
    );
    await openConversation(store);
    await user.click(screen.getByRole("button", { name: "Dictate a message" }));
    await user.click(
      await screen.findByRole("button", { name: "Stop dictation" }),
    );
    await waitFor(() => expect(bridge.callsTo("turn.submit")).toHaveLength(1));
    expect(bridge.callsTo("turn.submit")[0]).toMatchObject({
      text: "run the whole suite",
      conversationId: CONVERSATION_ID,
    });
    expect(screen.getByLabelText("Message to the lead")).toHaveValue("");
  });

  test("Enter stops and Esc cancels without transcribing", async () => {
    const bridge = scripted();
    bridge.voice.transcribe.mockResolvedValue(transcript("hello there"));
    const recorder = fakeRecorder();
    const user = userEvent.setup();
    const { store } = await mountVoice(
      bridge,
      { createRecorder: () => recorder },
      () => <Composer />,
    );
    await openConversation(store);

    await user.click(screen.getByRole("button", { name: "Dictate a message" }));
    await screen.findByText("Recording");
    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(screen.queryByText("Recording")).not.toBeInTheDocument(),
    );
    expect(recorder.cancel).toHaveBeenCalled();
    expect(bridge.voice.transcribe).not.toHaveBeenCalled();

    await user.click(screen.getByLabelText("Message to the lead"));
    await user.click(screen.getByRole("button", { name: "Dictate a message" }));
    await screen.findByText("Recording");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(screen.getByLabelText("Message to the lead")).toHaveValue(
        "hello there",
      ),
    );
    // Enter stopped the dictation; it did not send the message.
    expect(bridge.callsTo("turn.submit")).toHaveLength(0);
  });

  test("Ctrl+Shift+Space toggles recording", async () => {
    const bridge = scripted();
    bridge.voice.transcribe.mockResolvedValue(transcript("toggled"));
    const user = userEvent.setup();
    const { store } = await mountVoice(
      bridge,
      { createRecorder: () => fakeRecorder() },
      () => <Composer />,
    );
    await openConversation(store);
    await user.keyboard("{Control>}{Shift>} {/Shift}{/Control}");
    await screen.findByText("Recording");
    await user.keyboard("{Control>}{Shift>} {/Shift}{/Control}");
    await waitFor(() =>
      expect(screen.getByLabelText("Message to the lead")).toHaveValue(
        "toggled",
      ),
    );
  });

  test("a clip under 300 ms is discarded silently", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    const { store } = await mountVoice(
      bridge,
      { createRecorder: () => fakeRecorder(null) },
      () => <Composer />,
    );
    await openConversation(store);
    await user.click(screen.getByRole("button", { name: "Dictate a message" }));
    await user.click(
      await screen.findByRole("button", { name: "Stop dictation" }),
    );
    await waitFor(() =>
      expect(screen.queryByText("Recording")).not.toBeInTheDocument(),
    );
    expect(bridge.voice.transcribe).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  test("a failure shows its code in the composer error style", async () => {
    const bridge = scripted();
    bridge.voice.transcribe.mockRejectedValue({
      code: "LZ-VOICE-ENGINE-MISSING",
      message: "The speech model is not installed.",
    });
    const user = userEvent.setup();
    const { store } = await mountVoice(
      bridge,
      { createRecorder: () => fakeRecorder() },
      () => <Composer />,
    );
    await openConversation(store);
    await user.click(screen.getByRole("button", { name: "Dictate a message" }));
    await user.click(
      await screen.findByRole("button", { name: "Stop dictation" }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("LZ-VOICE-ENGINE-MISSING");
    expect(alert).toHaveTextContent("The speech model is not installed.");
    expect(alert).toHaveClass("composer__error");
  });

  test("a blocked microphone is reported with its code", async () => {
    const bridge = scripted();
    const recorder = fakeRecorder();
    recorder.start = vi.fn(async () => {
      throw { code: "LZ-VOICE-MIC", message: "Microphone access is blocked." };
    });
    const user = userEvent.setup();
    const { store, voice } = await mountVoice(
      bridge,
      { createRecorder: () => recorder },
      () => <Composer />,
    );
    await openConversation(store);
    await user.click(screen.getByRole("button", { name: "Dictate a message" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("LZ-VOICE-MIC");
    expect(voice.getSnapshot().phase).toBe("idle");
  });

  test("an external engine shows its host beside the microphone", async () => {
    const bridge = scripted(
      voiceState({
        stt: { engine: "endpoint", model: "base.en", language: "en" },
      }),
    );
    const { store } = await mountVoice(bridge, {}, () => <Composer />);
    await openConversation(store);
    expect(screen.getByText("External · api.openai.com")).toBeInTheDocument();
  });

  test("with speech to text off the mic is replaced by a link to Voice settings", async () => {
    const bridge = scripted(
      voiceState({ stt: { engine: "off", model: "base.en", language: "en" } }),
    );
    const user = userEvent.setup();
    const { store } = await mountVoice(bridge, {}, () => <Composer />);
    await openConversation(store);
    expect(
      screen.queryByRole("button", { name: "Dictate a message" }),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: /Speech to text is off/ }),
    );
    expect(store.getSnapshot().route).toEqual({
      kind: "settings",
      section: "voice",
    });
  });
});

describe("global push-to-talk", () => {
  const overlays = (bridge: FakeBridge) =>
    bridge.voice.overlay.mock.calls.map(([update]) => update);

  test("press records with the overlay, release transcribes as dictation and inserts", async () => {
    const bridge = scripted();
    bridge.voice.transcribe.mockResolvedValue(transcript("deploy to staging"));
    const recorder = fakeRecorder();
    const { voice } = await mountVoice(bridge, {
      createRecorder: () => recorder,
    });
    await act(async () => {
      bridge.firePtt("pressed");
    });
    await waitFor(() => expect(voice.getSnapshot().phase).toBe("recording"));
    expect(overlays(bridge)[0]).toMatchObject({ state: "recording", level: 0 });

    await act(async () => {
      recorder.level(0.6);
    });
    expect(
      overlays(bridge).some((u) => u.state === "recording" && u.level === 0.6),
    ).toBe(true);

    await act(async () => {
      bridge.firePtt("released");
    });
    await waitFor(() =>
      expect(bridge.voice.insertText).toHaveBeenCalledWith("deploy to staging"),
    );
    expect(bridge.voice.transcribe).toHaveBeenCalledWith(CLIP.wav, "dictation");
    const states = overlays(bridge).map((u) => u.state);
    expect(states).toContain("transcribing");
    expect(overlays(bridge).at(-1)).toMatchObject({
      state: "done",
      detail: "Inserted",
    });
    expect(voice.getSnapshot().phase).toBe("idle");
  });

  test("an insert refused while a conversation is open lands in its composer", async () => {
    const bridge = scripted();
    bridge.voice.transcribe.mockResolvedValue(transcript("from the hotkey"));
    bridge.voice.insertText.mockRejectedValue({
      code: "LZ-VOICE-INSERT-REFUSED",
      message: "The foreground window is Lazoo.",
    });
    const { store } = await mountVoice(
      bridge,
      { createRecorder: () => fakeRecorder() },
      () => <Composer />,
    );
    await openConversation(store);
    await act(async () => {
      bridge.firePtt("pressed");
    });
    await act(async () => {
      bridge.firePtt("released");
    });
    await waitFor(() =>
      expect(screen.getByLabelText("Message to the lead")).toHaveValue(
        "from the hotkey",
      ),
    );
    expect(overlays(bridge).at(-1)).toMatchObject({ state: "done" });
  });

  test("the clipboard target copies and flashes Copied", async () => {
    const bridge = scripted(voiceState({ dictationTarget: "clipboard" }));
    bridge.voice.transcribe.mockResolvedValue(transcript("copy me"));
    const clipboard = vi.fn(async () => undefined);
    await mountVoice(bridge, {
      createRecorder: () => fakeRecorder(),
      clipboard,
    });
    await act(async () => {
      bridge.firePtt("pressed");
    });
    await act(async () => {
      bridge.firePtt("released");
    });
    await waitFor(() => expect(clipboard).toHaveBeenCalledWith("copy me"));
    expect(bridge.voice.insertText).not.toHaveBeenCalled();
    expect(overlays(bridge).at(-1)).toMatchObject({
      state: "done",
      detail: "Copied",
    });
  });

  test("the conversation target sends to the open lead, or copies when none is open", async () => {
    const bridge = scripted(voiceState({ dictationTarget: "conversation" }));
    bridge.voice.transcribe.mockResolvedValue(transcript("status please"));
    const clipboard = vi.fn(async () => undefined);
    const { store } = await mountVoice(bridge, {
      createRecorder: () => fakeRecorder(),
      clipboard,
    });
    // No conversation open: copied, with a toast.
    await act(async () => {
      bridge.firePtt("pressed");
    });
    await act(async () => {
      bridge.firePtt("released");
    });
    await waitFor(() =>
      expect(clipboard).toHaveBeenCalledWith("status please"),
    );
    expect(store.getSnapshot().toasts.map((t) => t.title)).toContain(
      "No conversation is open",
    );

    await openConversation(store);
    await act(async () => {
      bridge.firePtt("pressed");
    });
    await act(async () => {
      bridge.firePtt("released");
    });
    await waitFor(() => expect(bridge.callsTo("turn.submit")).toHaveLength(1));
    expect(bridge.callsTo("turn.submit")[0]).toMatchObject({
      text: "status please",
    });
    await waitFor(() =>
      expect(overlays(bridge).at(-1)).toMatchObject({
        state: "done",
        detail: "Sent",
      }),
    );
  });

  test("a failure flashes the error on the overlay", async () => {
    const bridge = scripted();
    bridge.voice.transcribe.mockRejectedValue({
      code: "LZ-VOICE-TIMEOUT",
      message: "Transcription took too long.",
    });
    await mountVoice(bridge, { createRecorder: () => fakeRecorder() });
    await act(async () => {
      bridge.firePtt("pressed");
    });
    await act(async () => {
      bridge.firePtt("released");
    });
    await waitFor(() =>
      expect(overlays(bridge).at(-1)).toMatchObject({
        state: "error",
        detail: "Transcription took too long.",
      }),
    );
  });
});

describe("overlay window", () => {
  test("App renders only the pill when the window is voice-overlay", async () => {
    const bridge = scripted();
    bridge.setWindowLabel("voice-overlay");
    const store = new Store(bridge, { statusPollMs: 0 });
    const { container } = render(<App store={store} bridge={bridge} />);
    expect(container.querySelector(".voice-pill")).toBeInTheDocument();
    expect(
      screen.queryByLabelText("Open command palette"),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("main")).not.toBeInTheDocument();
    expect(bridge.callsTo("system.status")).toHaveLength(0);
    expect(document.documentElement.dataset.overlay).toBe("true");
  });

  test("renders each state the main window pushes", async () => {
    const bridge = createFakeBridge();
    const { container } = render(<VoiceOverlay bridge={bridge} />);
    const pill = container.querySelector(".voice-pill")!;
    await waitFor(() => expect(bridge.voice.onOverlay).toHaveBeenCalled());
    expect(pill).toHaveAttribute("data-state", "hidden");

    await act(async () => {
      bridge.fireOverlay({ state: "recording", detail: null, level: 0.4 });
    });
    expect(pill).toHaveAttribute("data-state", "recording");
    expect(
      within(pill as HTMLElement).getByText("Recording"),
    ).toBeInTheDocument();
    expect(pill.querySelector("canvas.level-meter")).toBeInTheDocument();

    // Level-only updates do not change the label or remount the meter.
    const canvas = pill.querySelector("canvas");
    await act(async () => {
      bridge.fireOverlay({ state: "recording", detail: null, level: 0.9 });
    });
    expect(pill.querySelector("canvas")).toBe(canvas);

    await act(async () => {
      bridge.fireOverlay({ state: "transcribing", detail: null, level: null });
    });
    expect(screen.getByText("Transcribing")).toBeInTheDocument();
    expect(pill.querySelector("canvas")).not.toBeInTheDocument();

    await act(async () => {
      bridge.fireOverlay({ state: "done", detail: "Copied", level: null });
    });
    expect(screen.getByText("Copied")).toBeInTheDocument();

    await act(async () => {
      bridge.fireOverlay({
        state: "error",
        detail: "Nothing heard",
        level: null,
      });
    });
    expect(screen.getByText("Nothing heard")).toBeInTheDocument();
    expect(pill).toHaveAttribute("data-state", "error");

    await act(async () => {
      bridge.fireOverlay({ state: "hidden", detail: null, level: null });
    });
    expect(pill).toHaveAttribute("hidden");
  });
});

describe("Voice settings", () => {
  async function openSettings(bridge: FakeBridge) {
    const user = userEvent.setup();
    const mounted = await mountVoice(bridge, {}, () => <SettingsPage />);
    await act(async () => {
      mounted.store.navigate({ kind: "settings", section: "voice" });
    });
    await screen.findByRole("heading", { name: "Speech to text" });
    return { user, ...mounted };
  }

  test("is a section in the Workspace group, after General", async () => {
    const bridge = scripted();
    await openSettings(bridge);
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    const labels = within(nav)
      .getAllByRole("button")
      .map((button) => button.textContent);
    expect(labels.indexOf("Voice")).toBe(labels.indexOf("General") + 1);
    expect(screen.getByRole("button", { name: "Voice" })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  test("groups follow the spec and the privacy sentence is plain", async () => {
    const bridge = scripted();
    await openSettings(bridge);
    for (const name of [
      "Speech to text",
      "Cleaning",
      "Text to speech",
      "Endpoint",
      "Privacy",
      "Push-to-talk",
    ])
      expect(screen.getByRole("heading", { name })).toBeInTheDocument();
    expect(
      screen.getByText(
        "Audio is never stored. Nothing leaves this computer unless this is on.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("switch", { name: "Allow external processing" }),
    ).toHaveAttribute("aria-checked", "false");
    // Model catalogue with sizes.
    expect(screen.getByText("141 MiB")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Remove base.en" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Install tiny.en" }),
    ).toBeInTheDocument();
  });

  test("a change is saved against the revision it started from", async () => {
    const bridge = scripted();
    const saved = voiceState(
      { cleaning: "model", allowExternal: true },
      { revision: 4 },
    );
    bridge.voice.settingsSet.mockResolvedValue(saved);
    const { user } = await openSettings(bridge);
    expect(
      screen.queryByRole("button", { name: "Save changes" }),
    ).not.toBeInTheDocument();

    await user.click(
      within(
        screen.getByRole("group", { name: "Transcript cleaning" }),
      ).getByRole("button", { name: "Model" }),
    );
    await user.click(
      screen.getByRole("switch", { name: "Allow external processing" }),
    );
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(bridge.voice.settingsSet).toHaveBeenCalledTimes(1),
    );
    const [settings, revision] = bridge.voice.settingsSet.mock.calls[0]!;
    expect(revision).toBe(3);
    expect(settings).toMatchObject({
      cleaning: "model",
      allowExternal: true,
      stt: { engine: "whisper-local", model: "base.en" },
    });
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "Save changes" }),
      ).not.toBeInTheDocument(),
    );
  });

  test("a stale revision reloads the state and says so", async () => {
    const bridge = scripted();
    bridge.voice.settingsSet.mockRejectedValue({
      code: "LZ-VOICE-REVISION",
      message: "Voice settings changed.",
    });
    const { user } = await openSettings(bridge);
    // What is true now, on the next read.
    bridge.voice.state.mockResolvedValue(
      voiceState({ cleaning: "off" }, { revision: 9 }),
    );
    await user.click(
      within(
        screen.getByRole("group", { name: "Transcript cleaning" }),
      ).getByRole("button", { name: "Model" }),
    );
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("LZ-VOICE-REVISION");
    expect(
      screen.getByText(/changed since you opened this page/),
    ).toBeInTheDocument();
    // The draft is the reloaded state, so there is nothing left to save.
    expect(
      within(
        screen.getByRole("group", { name: "Transcript cleaning" }),
      ).getByRole("button", { name: "Off" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(bridge.voice.state.mock.calls.length).toBeGreaterThan(1);
  });

  test("the API key is sent once and the field is cleared", async () => {
    const bridge = scripted(voiceState({ cleaning: "model" }));
    bridge.voice.keySet.mockResolvedValue(
      voiceState({ cleaning: "model" }, { keySet: true }),
    );
    const { user } = await openSettings(bridge);
    const field = screen.getByLabelText("API key");
    expect(field).toHaveAttribute("type", "password");
    await user.type(field, "sk-secret-value");
    await user.click(screen.getByRole("button", { name: "Save key" }));
    await waitFor(() =>
      expect(bridge.voice.keySet).toHaveBeenCalledWith("sk-secret-value"),
    );
    await waitFor(() => expect(field).toHaveValue(""));
    // A saved key is never displayed, only acknowledged.
    expect(
      await screen.findByRole("button", { name: "Rotate key" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/never shown/)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("sk-secret-value");
  });

  test("clearing the key calls voice_key_clear", async () => {
    const bridge = scripted(
      voiceState({ cleaning: "model" }, { keySet: true }),
    );
    bridge.voice.keyClear.mockResolvedValue(
      voiceState({ cleaning: "model" }, { keySet: false }),
    );
    const { user } = await openSettings(bridge);
    await user.click(screen.getByRole("button", { name: "Clear key" }));
    await waitFor(() => expect(bridge.voice.keyClear).toHaveBeenCalled());
  });

  test("model install shows progress fed by the channel, then installs", async () => {
    const bridge = scripted();
    let finish!: (state: VoiceState) => void;
    bridge.voice.whisperInstall.mockImplementation(
      (_model, onProgress) =>
        new Promise<VoiceState>((resolve) => {
          onProgress({ stage: "model", received: 25, total: 100 });
          finish = resolve;
        }),
    );
    const { user } = await openSettings(bridge);
    await user.click(screen.getByRole("button", { name: "Install tiny.en" }));
    const bar = await screen.findByRole("progressbar", {
      name: "Installing tiny.en",
    });
    expect(bar).toHaveAttribute("aria-valuenow", "25");
    expect(bar).toHaveAttribute("aria-valuetext", "Downloading the model");
    expect(bridge.voice.whisperInstall.mock.calls[0]![0]).toBe("tiny.en");
    await act(async () => {
      const next = voiceState();
      next.whisper.catalog[0] = {
        model: "tiny.en",
        bytes: 77_704_715,
        installed: true,
      };
      next.whisper.installedModels = ["base.en", "tiny.en"];
      finish(next);
    });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Remove tiny.en" }),
      ).toBeInTheDocument(),
    );
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  test("the hotkey field records a combination and shows a registration error", async () => {
    const bridge = scripted(
      voiceState(
        {},
        {
          hotkeyRegistered: false,
          hotkeyError: "Ctrl+Alt+Space is taken by another app.",
        },
      ),
    );
    const { user } = await openSettings(bridge);
    expect(screen.getByRole("alert")).toHaveTextContent("taken by another app");
    const field = screen.getByLabelText("Push-to-talk hotkey");
    expect(field).toHaveValue("Ctrl+Alt+Space");
    await user.click(field);
    expect(field).toHaveValue("Press a combination");
    await user.keyboard("{Control>}{Shift>}k{/Shift}{/Control}");
    expect(field).toHaveValue("Ctrl+Shift+K");
    // The palette shortcut did not fire while a combination was chosen.
    expect(
      screen.getByRole("button", { name: "Save changes" }),
    ).toBeInTheDocument();
  });

  test("an external endpoint is marked and blocked until external processing is allowed", async () => {
    const bridge = scripted(
      voiceState({
        cleaning: "model",
        endpoint: {
          ...DEFAULT_VOICE_SETTINGS.endpoint,
          baseUrl: "https://api.groq.com/openai/v1",
        },
      }),
    );
    await openSettings(bridge);
    expect(
      screen.getAllByText("External · api.groq.com").length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByText(/It will be refused until this is on/),
    ).toBeInTheDocument();
  });

  test("a loopback endpoint is not marked external", async () => {
    const bridge = scripted(
      voiceState({
        cleaning: "model",
        endpoint: {
          ...DEFAULT_VOICE_SETTINGS.endpoint,
          baseUrl: "http://127.0.0.1:8000/v1",
        },
      }),
    );
    await openSettings(bridge);
    expect(screen.queryByText(/^External ·/)).not.toBeInTheDocument();
    expect(screen.getByText(/nothing leaves it/)).toBeInTheDocument();
  });
});

describe("read replies aloud", () => {
  const record = (id: string, state: "accepted" | "completed", text: string) =>
    turnRecord({
      id,
      state,
      events: [
        {
          sequence: 1,
          kind: "text-delta",
          provider: "codex",
          nativeSessionId: "t",
          nativeTurnId: "t",
          payload: { text },
          createdAt: "2026-10-31T10:00:00Z",
        },
      ],
    });

  const OLD = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1";
  const NEW = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2";
  const WORKER_TURN = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3";

  test("speaks the lead's new reply as it grows, flushes at completion, and skips history and workers", async () => {
    const bridge = scripted(voiceState({ readReplies: true }));
    const speech = fakeSpeech();
    bridge.on("turn.show", (params: unknown) => {
      const id = (params as { id: string }).id;
      if (id === OLD)
        return record(OLD, "completed", "An old reply. Not spoken.");
      if (id === WORKER_TURN)
        return record(WORKER_TURN, "accepted", "Worker chatter. Never spoken.");
      return current();
    });
    let current = () => record(NEW, "accepted", "");
    const base = conversation({
      turns: [turnSummary({ id: OLD, sequence: 1, state: "completed" })],
    });
    bridge.on("workspace.conversation", base);
    const { store } = await mountVoice(bridge, { speech: speech.deps }, () => (
      <ReadAloud />
    ));
    await openConversation(store);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(speech.spoken).toEqual([]);

    // A new lead turn, mid-reply, plus a worker turn that must stay silent.
    const turns = [
      turnSummary({ id: OLD, sequence: 1, state: "completed" }),
      turnSummary({ id: NEW, sequence: 2, state: "accepted" }),
      turnSummary({
        id: WORKER_TURN,
        sequence: 3,
        taskId: TASK_ID,
        state: "accepted",
      }),
    ];
    current = () =>
      record(NEW, "accepted", "Reproduced locally. The WAL is shared. Next we");
    bridge.on(
      "workspace.conversation",
      conversation({ turns, eventHighWater: 50 }),
    );
    await act(async () => {
      await store.refresh("conversation");
    });
    await waitFor(() =>
      expect(speech.spoken).toEqual(["Reproduced locally. The WAL is shared."]),
    );

    // More text, then completion: the tail is flushed once.
    current = () =>
      record(
        NEW,
        "completed",
        "Reproduced locally. The WAL is shared. Next we fix it",
      );
    bridge.on(
      "workspace.conversation",
      conversation({
        turns: turns.map((t) =>
          t.id === NEW ? { ...t, state: "completed" as const, revision: 2 } : t,
        ),
        eventHighWater: 51,
      }),
    );
    await act(async () => {
      await store.refresh("conversation");
    });
    await waitFor(() =>
      expect(speech.spoken).toEqual([
        "Reproduced locally. The WAL is shared.",
        "Next we fix it",
      ]),
    );
    expect(
      bridge.callsTo("turn.show").map((p) => (p as { id: string }).id),
    ).not.toContain(WORKER_TURN);
  });

  test("stays silent when read replies is off", async () => {
    const bridge = scripted(voiceState({ readReplies: false }));
    const speech = fakeSpeech();
    bridge.on("turn.show", record(NEW, "completed", "Should not be read."));
    bridge.on("workspace.conversation", conversation({ turns: [] }));
    const { store } = await mountVoice(bridge, { speech: speech.deps }, () => (
      <ReadAloud />
    ));
    await openConversation(store);
    bridge.on(
      "workspace.conversation",
      conversation({
        turns: [turnSummary({ id: NEW, sequence: 2, state: "completed" })],
        eventHighWater: 60,
      }),
    );
    await act(async () => {
      await store.refresh("conversation");
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(speech.spoken).toEqual([]);
  });
});

describe("command palette", () => {
  test("offers Voice settings, a read-aloud toggle and Stop speaking", async () => {
    const bridge = scripted(voiceState({ readReplies: false }));
    bridge.voice.settingsSet.mockResolvedValue(
      voiceState({ readReplies: true }, { revision: 4 }),
    );
    const speech = fakeSpeech();
    // Speech that never finishes, so "speaking" holds.
    speech.deps.speakSystem = () => ({
      done: new Promise<void>(() => undefined),
      cancel: () => undefined,
    });
    const user = userEvent.setup();
    const { store, voice } = await mountVoice(
      bridge,
      { speech: speech.deps },
      () => <CommandPalette />,
    );
    expect(
      screen.getByRole("option", { name: /Voice settings/ }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: /Stop speaking/ }),
    ).not.toBeInTheDocument();

    await act(async () => {
      voice.speak("Hello there.");
    });
    expect(
      await screen.findByRole("option", { name: /Stop speaking/ }),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("option", { name: /Read replies aloud/ }),
    );
    await waitFor(() => expect(bridge.voice.settingsSet).toHaveBeenCalled());
    expect(bridge.voice.settingsSet.mock.calls[0]![0].readReplies).toBe(true);
    expect(bridge.voice.settingsSet.mock.calls[0]![1]).toBe(3);

    await user.click(screen.getByRole("option", { name: /Voice settings/ }));
    expect(store.getSnapshot().route).toEqual({
      kind: "settings",
      section: "voice",
    });
  });
});
