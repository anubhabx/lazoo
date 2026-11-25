import { createContext, useContext, useSyncExternalStore } from "react";

import type { VoiceController, VoiceSnapshot } from "./controller";

const VoiceContext = createContext<VoiceController | null>(null);

export const VoiceProvider = VoiceContext.Provider;

export function useVoice(): VoiceController {
  const voice = useContext(VoiceContext);
  if (!voice) throw new Error("VoiceProvider is missing");
  return voice;
}

export function useVoiceState(): VoiceSnapshot {
  const voice = useVoice();
  return useSyncExternalStore(
    voice.subscribe,
    voice.getSnapshot,
    voice.getSnapshot,
  );
}
