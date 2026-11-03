import { describe, expect, test } from "vitest";

import { hotkeyFromEvent, type KeyLike } from "./hotkey";

const key = (code: string, mods: Partial<KeyLike> = {}): KeyLike => ({
  key: code,
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

describe("hotkeyFromEvent", () => {
  test("names a combination with modifiers in a fixed order", () => {
    expect(hotkeyFromEvent(key("Space", { ctrlKey: true, altKey: true }))).toBe(
      "Ctrl+Alt+Space",
    );
    expect(
      hotkeyFromEvent(key("KeyD", { shiftKey: true, ctrlKey: true })),
    ).toBe("Ctrl+Shift+D");
    expect(hotkeyFromEvent(key("Digit7", { altKey: true }))).toBe("Alt+7");
    expect(hotkeyFromEvent(key("ArrowUp", { ctrlKey: true }))).toBe("Ctrl+Up");
    expect(hotkeyFromEvent(key("KeyV", { metaKey: true, altKey: true }))).toBe(
      "Alt+Super+V",
    );
  });

  test("waits while only modifiers are held", () => {
    expect(hotkeyFromEvent(key("ControlLeft", { ctrlKey: true }))).toBeNull();
    expect(hotkeyFromEvent(key("ShiftLeft", { shiftKey: true }))).toBeNull();
  });

  test("a bare letter is refused but a function key may stand alone", () => {
    expect(hotkeyFromEvent(key("KeyA"))).toBeNull();
    expect(hotkeyFromEvent(key("Space"))).toBeNull();
    expect(hotkeyFromEvent(key("F9"))).toBe("F9");
    expect(hotkeyFromEvent(key("F13", { ctrlKey: true }))).toBe("Ctrl+F13");
  });

  test("keys that cannot be hotkeys are ignored", () => {
    expect(hotkeyFromEvent(key("Escape", { ctrlKey: true }))).toBeNull();
    expect(hotkeyFromEvent(key("Backquote", { ctrlKey: true }))).toBeNull();
  });
});
