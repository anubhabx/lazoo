// Settings, Voice. Edits are a draft saved against the revision they started
// from (`LZ-VOICE-REVISION` reloads what is true); model installs and the API
// key are their own actions, applied at once.

import { useEffect, useRef, useState, type FormEvent } from "react";

import {
  Badge,
  Button,
  Field,
  Segmented,
  StatusWord,
  Switch,
  Page,
  PageHeader,
  Section,
  Skeleton,
} from "../components/primitives";
import { useBridge, useStore } from "../lib/context";
import type { RpcFailure } from "../lib/protocol";
import { formatBytes } from "../lib/util";
import { useVoice, useVoiceState } from "../lib/voice/context";
import { hotkeyFromEvent } from "../lib/voice/hotkey";
import {
  WHISPER_LABELS,
  endpointHost,
  isLoopbackUrl,
  type Cleaning,
  type DictationTarget,
  type SttEngine,
  type TtsEngine,
  type VoiceSettings,
  type VoiceState,
  type WhisperModel,
  type WhisperProgress,
} from "../lib/voice/types";
import { Failure } from "../lib/useRead";

const LANGUAGES: [string, string][] = [
  ["auto", "Detect automatically"],
  ["en", "English"],
  ["de", "German"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["hi", "Hindi"],
  ["it", "Italian"],
  ["ja", "Japanese"],
  ["ko", "Korean"],
  ["nl", "Dutch"],
  ["pt", "Portuguese"],
  ["ru", "Russian"],
  ["zh", "Chinese"],
];

const CLEANING_HELP: Record<Cleaning, string> = {
  off: "The transcript is used exactly as heard.",
  basic:
    "Removes fillers and stutters, fixes spacing and capitals, and ends with a full stop. Runs on this computer.",
  model:
    "A language model tidies the transcript through the endpoint. If it fails, basic cleaning is used.",
};

const PREVIEW_TEXT = "This is how Lazoo will read replies aloud.";

function VoiceSkeleton() {
  return (
    <div className="list" aria-busy="true">
      {[0, 1, 2].map((index) => (
        <div key={index} className="item">
          <Skeleton width={8} height={8} radius={4} />
          <div className="item__main">
            <Skeleton width="30%" height={12} />
            <Skeleton width="60%" height={10} />
          </div>
        </div>
      ))}
    </div>
  );
}

export function VoicePage() {
  const { state, stateError } = useVoiceState();
  return (
    <Page>
      <PageHeader
        title="Voice"
        description="Dictate into a conversation, hear replies read aloud, or dictate into any app with a global hotkey. Voice only produces text; it never answers an approval."
      />
      <Failure error={stateError} />
      {state?.microphoneBlocked ? <MicrophoneBlocked /> : null}
      {state ? <VoiceForm state={state} /> : <VoiceSkeleton />}
    </Page>
  );
}

function useSystemVoices(): string[] {
  const read = () =>
    typeof window !== "undefined" && window.speechSynthesis
      ? window.speechSynthesis.getVoices().map((voice) => voice.name)
      : [];
  const [names, setNames] = useState<string[]>(read);
  useEffect(() => {
    const synth = typeof window !== "undefined" ? window.speechSynthesis : null;
    if (!synth?.addEventListener) return;
    const update = () => setNames(read());
    synth.addEventListener("voiceschanged", update);
    update();
    return () => synth.removeEventListener("voiceschanged", update);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return names;
}

/** Windows owns this switch; Lazoo only says which one is off and opens the page. */
function MicrophoneBlocked() {
  const bridge = useBridge();
  return (
    <section
      className="page-section voice-blocked"
      role="note"
      aria-label="Microphone blocked"
    >
      <div className="voice-row">
        <span className="voice-row__text">
          <span className="voice-row__label">
            Windows is blocking the microphone
          </span>
          <span className="meta">
            Turn on Microphone access and Let desktop apps access your
            microphone in Privacy &amp; security, then come back.
          </span>
        </span>
        <Button onClick={() => void bridge.voice.openMicrophoneSettings()}>
          Open Windows settings
        </Button>
      </div>
    </section>
  );
}

function External({ host }: { host: string | null }) {
  return (
    <Badge tone="warning" title="Audio or text leaves this computer">
      External · {host ?? "unknown host"}
    </Badge>
  );
}

function VoiceForm({ state }: { state: VoiceState }) {
  const voice = useVoice();
  const store = useStore();
  const [draft, setDraft] = useState<VoiceSettings>(state.settings);
  const base = useRef({ revision: state.revision, saved: state.settings });
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<RpcFailure | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [showEndpoint, setShowEndpoint] = useState(false);
  const voices = useSystemVoices();

  const same = (a: VoiceSettings, b: VoiceSettings) =>
    JSON.stringify(a) === JSON.stringify(b);
  const dirty = !same(draft, state.settings);

  // A state that moved while nothing was being edited is adopted quietly.
  useEffect(() => {
    if (!same(draft, base.current.saved)) return;
    setDraft(state.settings);
    base.current = { revision: state.revision, saved: state.settings };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.settings]);

  function edit(change: (value: VoiceSettings) => VoiceSettings) {
    setDraft(change);
    setNotice(null);
  }

  async function save() {
    setSaving(true);
    setSaveError(null);
    setNotice(null);
    const outcome = await voice.saveSettings(draft, base.current.revision);
    setSaving(false);
    if (outcome.ok) {
      const next = voice.getSnapshot().state;
      if (next) {
        base.current = { revision: next.revision, saved: next.settings };
        setDraft(next.settings);
      }
      store.toast({ tone: "info", title: "Voice settings saved" });
      return;
    }
    if (outcome.failure.code === "LZ-VOICE-REVISION") {
      // `saveSettings` re-read the state; show what is true now.
      const next = voice.getSnapshot().state;
      if (next) {
        base.current = { revision: next.revision, saved: next.settings };
        setDraft(next.settings);
      }
      setNotice(
        "These settings changed since you opened this page. They are reloaded; review them and save again.",
      );
    }
    setSaveError(outcome.failure);
  }

  function revert() {
    setDraft(state.settings);
    setSaveError(null);
    setNotice(null);
  }

  const endpointUsed =
    draft.stt.engine === "endpoint" ||
    draft.tts.engine === "endpoint" ||
    draft.cleaning === "model";
  const host = endpointHost(draft.endpoint.baseUrl);
  const external = !isLoopbackUrl(draft.endpoint.baseUrl);
  const sttExternal = draft.stt.engine === "endpoint" && external;
  const cleaningExternal = draft.cleaning === "model" && external;
  const ttsExternal = draft.tts.engine === "endpoint" && external;
  const anyExternal = sttExternal || cleaningExternal || ttsExternal;

  return (
    <>
      <Section
        label="Speech to text"
        title="Speech to text"
        actions={sttExternal ? <External host={host} /> : null}
      >
        <div className="form-grid">
          <Field label="Engine" htmlFor="voice-stt-engine">
            <select
              id="voice-stt-engine"
              className="select"
              value={draft.stt.engine}
              onChange={(event) =>
                edit((value) => ({
                  ...value,
                  stt: {
                    ...value.stt,
                    engine: event.target.value as SttEngine,
                  },
                }))
              }
            >
              <option value="whisper-local">Whisper on this computer</option>
              <option value="endpoint">Endpoint</option>
              <option value="off">Off</option>
            </select>
          </Field>
          {draft.stt.engine !== "off" ? (
            <Field
              label="Language"
              htmlFor="voice-language"
              hint={
                draft.stt.engine === "whisper-local" &&
                draft.stt.model.endsWith(".en")
                  ? "English-only models ignore this."
                  : undefined
              }
            >
              <select
                id="voice-language"
                className="select"
                value={draft.stt.language}
                onChange={(event) =>
                  edit((value) => ({
                    ...value,
                    stt: { ...value.stt, language: event.target.value },
                  }))
                }
              >
                {LANGUAGES.map(([code, label]) => (
                  <option key={code} value={code}>
                    {label}
                  </option>
                ))}
              </select>
            </Field>
          ) : null}
        </div>
        {draft.stt.engine === "whisper-local" ? (
          <WhisperModels
            state={state}
            selected={draft.stt.model}
            onSelect={(model) =>
              edit((value) => ({ ...value, stt: { ...value.stt, model } }))
            }
          />
        ) : null}
      </Section>

      <Section
        label="Cleaning"
        title="Cleaning"
        actions={cleaningExternal ? <External host={host} /> : null}
      >
        <Segmented<Cleaning>
          name="voice-cleaning"
          label="Transcript cleaning"
          value={draft.cleaning}
          options={[
            { value: "off", label: "Off" },
            { value: "basic", label: "Basic" },
            { value: "model", label: "Model" },
          ]}
          onChange={(cleaning) => edit((value) => ({ ...value, cleaning }))}
        />
        <p className="meta voice-help">{CLEANING_HELP[draft.cleaning]}</p>
      </Section>

      <Section
        label="Text to speech"
        title="Text to speech"
        actions={ttsExternal ? <External host={host} /> : null}
      >
        <div className="form-grid">
          <Field label="Engine" htmlFor="voice-tts-engine">
            <select
              id="voice-tts-engine"
              className="select"
              value={draft.tts.engine}
              onChange={(event) =>
                edit((value) => ({
                  ...value,
                  tts: {
                    ...value.tts,
                    engine: event.target.value as TtsEngine,
                  },
                }))
              }
            >
              <option value="system">Windows voices</option>
              <option value="endpoint">Endpoint</option>
              <option value="off">Off</option>
            </select>
          </Field>
          {draft.tts.engine === "system" ? (
            <Field label="Voice" htmlFor="voice-system-voice">
              <select
                id="voice-system-voice"
                className="select"
                value={draft.tts.voice}
                onChange={(event) =>
                  edit((value) => ({
                    ...value,
                    tts: { ...value.tts, voice: event.target.value },
                  }))
                }
              >
                <option value="">Windows default</option>
                {draft.tts.voice && !voices.includes(draft.tts.voice) ? (
                  <option value={draft.tts.voice}>
                    {draft.tts.voice} (not installed)
                  </option>
                ) : null}
                {voices.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
            </Field>
          ) : null}
        </div>
        {draft.tts.engine !== "off" ? (
          <div className="voice-rate">
            <label className="field__label" htmlFor="voice-rate">
              Speed
            </label>
            <input
              id="voice-rate"
              className="voice-range"
              type="range"
              min={0.5}
              max={2}
              step={0.1}
              value={draft.tts.rate}
              onChange={(event) =>
                edit((value) => ({
                  ...value,
                  tts: { ...value.tts, rate: Number(event.target.value) },
                }))
              }
            />
            <span className="mono voice-rate__value">
              {draft.tts.rate.toFixed(1)}×
            </span>
            <Button
              onClick={() => {
                voice.stopSpeaking();
                voice.speak(PREVIEW_TEXT, draft.tts);
              }}
            >
              Preview
            </Button>
          </div>
        ) : null}
        {draft.tts.engine === "endpoint" ? (
          <p className="meta voice-help">
            Preview with an endpoint voice uses the saved endpoint settings.
          </p>
        ) : null}
      </Section>

      <Section
        label="Endpoint"
        title="Endpoint"
        actions={endpointUsed && external ? <External host={host} /> : null}
      >
        {endpointUsed || showEndpoint ? (
          <EndpointFields
            draft={draft}
            keySet={state.keySet}
            edit={edit}
            external={external}
          />
        ) : (
          <>
            <p className="meta voice-help">
              Any OpenAI-compatible server: a hosted API, or one running on this
              computer. Used when speech to text, text to speech or cleaning is
              set to an endpoint.
            </p>
            <Button onClick={() => setShowEndpoint(true)}>
              Configure endpoint
            </Button>
          </>
        )}
      </Section>

      <Section label="Privacy" title="Privacy">
        <div className="voice-row">
          <span className="voice-row__text">
            <span className="voice-row__label">Allow external processing</span>
            <span className="meta">
              Audio is never stored. Nothing leaves this computer unless this is
              on.
            </span>
          </span>
          <Switch
            label="Allow external processing"
            checked={draft.allowExternal}
            onChange={(allowExternal) =>
              edit((value) => ({ ...value, allowExternal }))
            }
          />
        </div>
        {anyExternal && !draft.allowExternal ? (
          <p className="meta voice-warning" role="note">
            An engine is set to send data to {host ?? "an external host"}. It
            will be refused until this is on.
          </p>
        ) : null}
      </Section>

      <Section label="Push-to-talk" title="Push-to-talk">
        <div className="form-grid">
          <Field
            label="Hotkey"
            htmlFor="voice-hotkey"
            hint="Hold to talk, release to deliver. Backspace turns it off."
          >
            <HotkeyField
              value={draft.hotkey}
              onChange={(hotkey) => edit((value) => ({ ...value, hotkey }))}
            />
          </Field>
          <div className="field">
            <span className="field__label">Status</span>
            <span className="voice-status">
              {draft.hotkey !== state.settings.hotkey ? (
                <span className="meta">Save to apply</span>
              ) : state.hotkeyRegistered ? (
                <StatusWord tone="healthy" label="Registered" />
              ) : state.settings.hotkey ? (
                <StatusWord tone="failed" label="Not registered" />
              ) : (
                <StatusWord tone="dormant" label="Off" />
              )}
            </span>
          </div>
        </div>
        {state.hotkeyError && draft.hotkey === state.settings.hotkey ? (
          <p className="voice-error" role="alert">
            {state.hotkeyError}
          </p>
        ) : null}
        <div className="voice-block">
          <span className="field__label" id="voice-target-label">
            Dictated text goes to
          </span>
          <Segmented<DictationTarget>
            name="voice-target"
            label="Dictated text goes to"
            value={draft.dictationTarget}
            options={[
              { value: "insert", label: "The focused app" },
              { value: "clipboard", label: "The clipboard" },
              { value: "conversation", label: "The open conversation" },
            ]}
            onChange={(dictationTarget) =>
              edit((value) => ({ ...value, dictationTarget }))
            }
          />
        </div>
        <div className="voice-row">
          <span className="voice-row__text">
            <span className="voice-row__label">Review before send</span>
            <span className="meta">
              Dictating in a conversation puts the text in the message box so
              you can correct it, instead of sending it.
            </span>
          </span>
          <Switch
            label="Review before send"
            checked={draft.reviewBeforeSend}
            onChange={(reviewBeforeSend) =>
              edit((value) => ({ ...value, reviewBeforeSend }))
            }
          />
        </div>
      </Section>

      {notice ? (
        <p className="voice-warning" role="status">
          {notice}
        </p>
      ) : null}
      {saveError ? (
        <p className="voice-error" role="alert">
          <span className="mono">{saveError.code}</span> {saveError.message}
        </p>
      ) : null}
      {dirty ? (
        <div
          className="voice-savebar"
          role="group"
          aria-label="Unsaved changes"
        >
          <span className="meta">Unsaved changes</span>
          <span className="spacer" />
          <Button variant="ghost" onClick={revert} disabled={saving}>
            Revert
          </Button>
          <Button
            variant="primary"
            loading={saving}
            onClick={() => void save()}
          >
            Save changes
          </Button>
        </div>
      ) : null}
    </>
  );
}

// ---- Whisper models ------------------------------------------------------

const STAGE_LABEL: Record<WhisperProgress["stage"], string> = {
  binary: "Downloading the speech engine",
  model: "Downloading the model",
  verify: "Verifying",
  done: "Done",
};

function WhisperModels({
  state,
  selected,
  onSelect,
}: {
  state: VoiceState;
  selected: WhisperModel;
  onSelect: (model: WhisperModel) => void;
}) {
  const voice = useVoice();
  const [installing, setInstalling] = useState<{
    model: WhisperModel;
    progress: WhisperProgress | null;
  } | null>(null);
  const [error, setError] = useState<RpcFailure | null>(null);
  const [confirmAll, setConfirmAll] = useState(false);
  const { catalog, binaryInstalled, installedModels } = state.whisper;

  useEffect(() => {
    if (!confirmAll) return;
    const timer = setTimeout(() => setConfirmAll(false), 4000);
    return () => clearTimeout(timer);
  }, [confirmAll]);

  async function install(model: WhisperModel) {
    setError(null);
    setInstalling({ model, progress: null });
    const failure = await voice.installModel(model, (progress) =>
      setInstalling({ model, progress }),
    );
    setInstalling(null);
    if (failure) setError(failure);
  }

  async function remove(model: WhisperModel | null) {
    setError(null);
    setConfirmAll(false);
    const failure = await voice.removeModel(model);
    if (failure) setError(failure);
  }

  return (
    <div className="voice-models">
      <span className="field__label" id="voice-model-label">
        Model
      </span>
      <div role="radiogroup" aria-labelledby="voice-model-label">
        {catalog.map((entry) => {
          const isInstalled =
            entry.installed || installedModels.includes(entry.model);
          const busy = installing?.model === entry.model;
          const progress = busy ? installing.progress : null;
          const fraction =
            progress && progress.total > 0
              ? Math.min(1, progress.received / progress.total)
              : 0;
          return (
            <div className="voice-model" key={entry.model}>
              <label className="voice-model__pick">
                <input
                  type="radio"
                  name="voice-whisper-model"
                  checked={selected === entry.model}
                  onChange={() => onSelect(entry.model)}
                />
                <span className="voice-model__name">
                  {WHISPER_LABELS[entry.model]}
                </span>
                <span className="mono voice-model__id">{entry.model}</span>
              </label>
              <span className="meta voice-model__size">
                {formatBytes(entry.bytes)}
              </span>
              <span className="voice-model__state">
                {busy ? (
                  <span
                    className="voice-progress"
                    role="progressbar"
                    aria-label={`Installing ${entry.model}`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round(fraction * 100)}
                    aria-valuetext={
                      progress ? STAGE_LABEL[progress.stage] : "Starting"
                    }
                  >
                    <span className="meter" aria-hidden="true">
                      <span
                        className="meter__fill"
                        style={{ width: `${fraction * 100}%` }}
                      />
                    </span>
                    <span className="meta">
                      {progress ? STAGE_LABEL[progress.stage] : "Starting"}
                    </span>
                  </span>
                ) : isInstalled ? (
                  <Badge tone="success">installed</Badge>
                ) : null}
              </span>
              <span className="voice-model__action">
                {isInstalled ? (
                  <Button
                    variant="ghost"
                    aria-label={`Remove ${entry.model}`}
                    disabled={installing !== null}
                    onClick={() => void remove(entry.model)}
                  >
                    Remove
                  </Button>
                ) : (
                  <Button
                    aria-label={`Install ${entry.model}`}
                    loading={busy}
                    disabled={installing !== null}
                    onClick={() => void install(entry.model)}
                  >
                    Install
                  </Button>
                )}
              </span>
            </div>
          );
        })}
      </div>
      {!installedModels.includes(selected) ? (
        <p className="meta voice-warning">
          {WHISPER_LABELS[selected]} is not installed. Install it to dictate
          with it.
        </p>
      ) : null}
      {error ? (
        <p className="voice-error" role="alert">
          <span className="mono">{error.code}</span> {error.message}
        </p>
      ) : null}
      {binaryInstalled || installedModels.length ? (
        <div className="voice-models__all">
          <span className="meta">
            Models and the speech engine are stored on this computer.
          </span>
          <Button
            variant={confirmAll ? "danger" : "ghost"}
            disabled={installing !== null}
            onClick={() =>
              confirmAll ? void remove(null) : setConfirmAll(true)
            }
          >
            {confirmAll ? "Confirm remove all" : "Remove all"}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

// ---- Endpoint --------------------------------------------------------------

function EndpointFields({
  draft,
  keySet,
  edit,
  external,
}: {
  draft: VoiceSettings;
  keySet: boolean;
  edit: (change: (value: VoiceSettings) => VoiceSettings) => void;
  external: boolean;
}) {
  const voice = useVoice();
  const [key, setKey] = useState("");
  const [error, setError] = useState<RpcFailure | null>(null);
  const endpoint = draft.endpoint;
  const set = (patch: Partial<VoiceSettings["endpoint"]>) =>
    edit((value) => ({ ...value, endpoint: { ...value.endpoint, ...patch } }));

  async function submitKey(event: FormEvent) {
    event.preventDefault();
    if (!key) return;
    const submitted = key;
    // The secret leaves the field as it is sent, whatever the outcome.
    setKey("");
    setError(await voice.setKey(submitted));
  }

  return (
    <div className="stack">
      <div className="form-grid">
        <Field
          label="Base URL"
          htmlFor="voice-base-url"
          hint={
            external
              ? undefined
              : "A server on this computer: nothing leaves it."
          }
        >
          <input
            id="voice-base-url"
            className="input mono-input"
            value={endpoint.baseUrl}
            spellCheck={false}
            onChange={(event) => set({ baseUrl: event.target.value })}
          />
        </Field>
        <Field label="Speech voice" htmlFor="voice-speech-voice">
          <input
            id="voice-speech-voice"
            className="input mono-input"
            value={endpoint.speechVoice}
            spellCheck={false}
            onChange={(event) => set({ speechVoice: event.target.value })}
          />
        </Field>
        <Field label="Transcription model" htmlFor="voice-transcription-model">
          <input
            id="voice-transcription-model"
            className="input mono-input"
            value={endpoint.transcriptionModel}
            spellCheck={false}
            onChange={(event) =>
              set({ transcriptionModel: event.target.value })
            }
          />
        </Field>
        <Field label="Speech model" htmlFor="voice-speech-model">
          <input
            id="voice-speech-model"
            className="input mono-input"
            value={endpoint.speechModel}
            spellCheck={false}
            onChange={(event) => set({ speechModel: event.target.value })}
          />
        </Field>
        <Field label="Cleanup model" htmlFor="voice-cleanup-model">
          <input
            id="voice-cleanup-model"
            className="input mono-input"
            value={endpoint.cleanupModel}
            spellCheck={false}
            onChange={(event) => set({ cleanupModel: event.target.value })}
          />
        </Field>
      </div>
      <form className="voice-key" onSubmit={(event) => void submitKey(event)}>
        <Field
          label="API key"
          htmlFor="voice-api-key"
          hint={
            keySet
              ? "A key is saved for this Windows user. It is never shown."
              : "Protected by Windows for this user and never shown again."
          }
        >
          <input
            id="voice-api-key"
            className="input mono-input"
            type="password"
            autoComplete="new-password"
            placeholder={keySet ? "Enter a new key to replace it" : "Key"}
            value={key}
            onChange={(event) => setKey(event.target.value)}
          />
        </Field>
        <div className="voice-key__actions">
          <Button type="submit" disabled={!key}>
            {keySet ? "Rotate key" : "Save key"}
          </Button>
          {keySet ? (
            <Button
              variant="ghost"
              onClick={() => {
                setKey("");
                void voice.clearKey().then(setError);
              }}
            >
              Clear key
            </Button>
          ) : null}
        </div>
      </form>
      {error ? (
        <p className="voice-error" role="alert">
          <span className="mono">{error.code}</span> {error.message}
        </p>
      ) : null}
    </div>
  );
}

// ---- Hotkey capture ----------------------------------------------------------

function HotkeyField({
  value,
  onChange,
}: {
  value: string;
  onChange: (hotkey: string) => void;
}) {
  const [capturing, setCapturing] = useState(false);
  return (
    <input
      id="voice-hotkey"
      className="input mono-input voice-hotkey"
      readOnly
      aria-label="Push-to-talk hotkey"
      value={capturing ? "Press a combination" : value || "Off"}
      data-capturing={capturing || undefined}
      onFocus={() => setCapturing(true)}
      onBlur={() => setCapturing(false)}
      onKeyDown={(event) => {
        if (event.key === "Tab") return;
        // Lazoo's own shortcuts must not fire while a combination is chosen.
        event.preventDefault();
        event.stopPropagation();
        if (event.key === "Escape") {
          event.currentTarget.blur();
          return;
        }
        if (event.key === "Backspace" || event.key === "Delete") {
          onChange("");
          event.currentTarget.blur();
          return;
        }
        const combination = hotkeyFromEvent(event.nativeEvent);
        if (!combination) return;
        onChange(combination);
        event.currentTarget.blur();
      }}
    />
  );
}
