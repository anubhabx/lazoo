// Turns a keydown into the text form the desktop registers as the global
// push-to-talk hotkey: modifiers in a fixed order, then one key ("Ctrl+Alt+Space").

export interface KeyLike {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

const NAMED: Record<string, string> = {
  Space: "Space",
  Enter: "Enter",
  Tab: "Tab",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Insert: "Insert",
};

function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1]!;
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1]!;
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return NAMED[code] ?? null;
}

/**
 * The combination a keydown describes, or null while only modifiers are held
 * or the key cannot be a hotkey. A bare letter is refused: it would swallow
 * typing everywhere. Function keys may stand alone.
 */
export function hotkeyFromEvent(event: KeyLike): string | null {
  const key = keyName(event.code);
  if (!key) return null;
  const modifiers = [
    event.ctrlKey ? "Ctrl" : null,
    event.altKey ? "Alt" : null,
    event.shiftKey ? "Shift" : null,
    event.metaKey ? "Super" : null,
  ].filter((value): value is string => value !== null);
  if (!modifiers.length && !/^F\d+$/.test(key)) return null;
  return [...modifiers, key].join("+");
}
