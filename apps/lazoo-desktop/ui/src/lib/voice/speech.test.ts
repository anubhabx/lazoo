import { describe, expect, test, vi } from "vitest";

import {
  FETCH_AHEAD,
  SpeechQueue,
  toSpeakable,
  type Playback,
  type SpeechDeps,
} from "./speech";

/** A playback the test finishes by hand. */
function manualPlayback() {
  let finish!: () => void;
  let fail!: (reason: unknown) => void;
  const cancel = vi.fn(() => finish());
  const done = new Promise<void>((resolve, reject) => {
    finish = resolve;
    fail = reject;
  });
  const playback: Playback & {
    finish: () => void;
    fail: (r: unknown) => void;
  } = { done, cancel, finish, fail };
  return playback;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function harness() {
  const spoken: string[] = [];
  const playbacks: ReturnType<typeof manualPlayback>[] = [];
  const fetches: {
    text: string;
    d: ReturnType<typeof deferred<ArrayBuffer>>;
  }[] = [];
  const played: ArrayBuffer[] = [];
  const deps: SpeechDeps = {
    speakSystem: (text) => {
      spoken.push(text);
      const playback = manualPlayback();
      playbacks.push(playback);
      return playback;
    },
    fetchAudio: (text) => {
      const d = deferred<ArrayBuffer>();
      fetches.push({ text, d });
      return d.promise;
    },
    playAudio: (audio) => {
      played.push(audio);
      const playback = manualPlayback();
      playbacks.push(playback);
      return playback;
    },
  };
  const onError = vi.fn();
  const queue = new SpeechQueue(deps, onError);
  return { queue, spoken, playbacks, fetches, played, onError };
}

const bytes = (value: number) => new Uint8Array([value]).buffer;

describe("SpeechQueue, system voice", () => {
  test("speaks chunks strictly in order, one at a time", async () => {
    const { queue, spoken, playbacks } = harness();
    queue.configure({ engine: "system", voice: "Zira", rate: 1.2 });
    queue.enqueue("one");
    queue.enqueue("two");
    queue.enqueue("three");
    await tick();
    expect(spoken).toEqual(["one"]);
    expect(queue.getSnapshot()).toEqual({ speaking: true, pending: 3 });
    playbacks[0]!.finish();
    await tick();
    expect(spoken).toEqual(["one", "two"]);
    playbacks[1]!.finish();
    await tick();
    playbacks[2]!.finish();
    await tick();
    expect(spoken).toEqual(["one", "two", "three"]);
    expect(queue.getSnapshot()).toEqual({ speaking: false, pending: 0 });
  });

  test("stop cancels what is playing and drops what is queued", async () => {
    const { queue, spoken, playbacks } = harness();
    queue.configure({ engine: "system", voice: "", rate: 1 });
    queue.enqueue("one");
    queue.enqueue("two");
    await tick();
    queue.stop();
    expect(playbacks[0]!.cancel).toHaveBeenCalled();
    await tick();
    expect(spoken).toEqual(["one"]);
    expect(queue.getSnapshot().speaking).toBe(false);
    // A new reply after Stop plays normally.
    queue.enqueue("three");
    await tick();
    expect(spoken).toEqual(["one", "three"]);
  });

  test("a playback failure stops the queue and reports once", async () => {
    const { queue, spoken, playbacks, onError } = harness();
    queue.configure({ engine: "system", voice: "", rate: 1 });
    queue.enqueue("one");
    queue.enqueue("two");
    await tick();
    playbacks[0]!.fail({ code: "LZ-VOICE-PLAYBACK", message: "no sound" });
    await tick();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0]![0]).toMatchObject({
      code: "LZ-VOICE-PLAYBACK",
    });
    expect(spoken).toEqual(["one"]);
    expect(queue.getSnapshot().speaking).toBe(false);
  });

  test("blank text is not queued", async () => {
    const { queue, spoken } = harness();
    queue.enqueue("   ");
    await tick();
    expect(spoken).toEqual([]);
  });
});

describe("SpeechQueue, endpoint voice", () => {
  test("fetches the head and at most two chunks ahead, and plays in order", async () => {
    const { queue, fetches, played, playbacks } = harness();
    queue.configure({ engine: "endpoint", voice: "", rate: 1 });
    for (const text of ["a", "b", "c", "d", "e"]) queue.enqueue(text);
    await tick();
    // The head plus FETCH_AHEAD: three requests, not five.
    expect(fetches.map((f) => f.text)).toEqual(["a", "b", "c"]);
    expect(fetches).toHaveLength(FETCH_AHEAD + 1);

    // Later chunks may arrive first; nothing plays out of order.
    fetches[2]!.d.resolve(bytes(3));
    fetches[1]!.d.resolve(bytes(2));
    await tick();
    expect(played).toHaveLength(0);
    fetches[0]!.d.resolve(bytes(1));
    await tick();
    expect(played.map((audio) => new Uint8Array(audio)[0])).toEqual([1]);

    // Finishing the head lets the window slide: "d" is requested now.
    playbacks[0]!.finish();
    await tick();
    expect(fetches.map((f) => f.text)).toEqual(["a", "b", "c", "d"]);
    expect(played.map((audio) => new Uint8Array(audio)[0])).toEqual([1, 2]);
    playbacks[1]!.finish();
    await tick();
    expect(fetches.map((f) => f.text)).toEqual(["a", "b", "c", "d", "e"]);
    expect(played.map((audio) => new Uint8Array(audio)[0])).toEqual([1, 2, 3]);
  });

  test("a failure ahead of the head is held until that chunk's turn", async () => {
    const { queue, fetches, played, onError } = harness();
    queue.configure({ engine: "endpoint", voice: "", rate: 1 });
    queue.enqueue("a");
    queue.enqueue("b");
    await tick();
    fetches[1]!.d.reject({ code: "LZ-VOICE-EXTERNAL-REFUSED", message: "no" });
    await tick();
    expect(onError).not.toHaveBeenCalled(); // "b" has not had its turn
    fetches[0]!.d.resolve(bytes(1));
    await tick();
    expect(played).toHaveLength(1);
  });

  test("a refused head is reported and nothing plays", async () => {
    const { queue, fetches, played, onError } = harness();
    queue.configure({ engine: "endpoint", voice: "", rate: 1 });
    queue.enqueue("a");
    queue.enqueue("b");
    await tick();
    fetches[0]!.d.reject({
      code: "LZ-VOICE-EXTERNAL-REFUSED",
      message: "External processing is off.",
    });
    await tick();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0]![0].code).toBe("LZ-VOICE-EXTERNAL-REFUSED");
    expect(played).toHaveLength(0);
    expect(queue.getSnapshot().speaking).toBe(false);
  });

  test("stop discards chunks that arrive late", async () => {
    const { queue, fetches, played } = harness();
    queue.configure({ engine: "endpoint", voice: "", rate: 1 });
    queue.enqueue("a");
    await tick();
    queue.stop();
    fetches[0]!.d.resolve(bytes(1));
    await tick();
    expect(played).toHaveLength(0);
  });
});

describe("toSpeakable", () => {
  test("drops markdown syntax and code fences", () => {
    expect(
      toSpeakable("**Done**: see [the docs](http://x.y) and `cargo test`."),
    ).toBe("Done: see the docs and cargo test.");
    expect(toSpeakable("# Heading\n- item one\n- item two")).toBe(
      "Heading item one item two",
    );
    expect(toSpeakable("```\nlet a = 1;\n```")).toBe("");
  });
});
