import { describe, expect, test } from "vitest";

import { MAX_CHUNK, SentenceChunker } from "./chunker";

function run(parts: string[]): string[] {
  const chunker = new SentenceChunker();
  const out: string[] = [];
  let text = "";
  for (const part of parts) {
    text += part;
    out.push(...chunker.push(text));
  }
  out.push(...chunker.flush());
  return out;
}

describe("SentenceChunker", () => {
  test("holds a sentence until its boundary is certain", () => {
    const chunker = new SentenceChunker();
    expect(chunker.push("Reproduced locally")).toEqual([]);
    // A final full stop could still be the start of 3.5: not stable yet.
    expect(chunker.push("Reproduced locally.")).toEqual([]);
    expect(chunker.push("Reproduced locally. Now")).toEqual([
      "Reproduced locally.",
    ]);
    expect(chunker.push("Reproduced locally. Now fixing it!")).toEqual([]);
    expect(chunker.flush()).toEqual(["Now fixing it!"]);
  });

  test.each([
    ["period", "One. Two. "],
    ["bang", "One! Two! "],
    ["question", "One? Two? "],
    ["colon", "One: Two: "],
  ])("splits at a %s followed by space", (_name, text) => {
    const chunker = new SentenceChunker();
    const stable = chunker.push(text);
    // Both sentences are packed into one chunk, the boundary is still honoured.
    expect(stable.join(" ")).toMatch(/^One\S Two\S$/);
    expect(chunker.flush()).toEqual([]);
  });

  test("a newline is a hard boundary and is never packed across", () => {
    const chunker = new SentenceChunker();
    expect(chunker.push("first line\nsecond line\nthird")).toEqual([
      "first line",
      "second line",
    ]);
    expect(chunker.flush()).toEqual(["third"]);
  });

  test("push returns only newly stable chunks", () => {
    const chunker = new SentenceChunker();
    const first = chunker.push("Alpha one.\nBeta two.\nGam");
    expect(first).toEqual(["Alpha one.", "Beta two."]);
    expect(chunker.push("Alpha one.\nBeta two.\nGamma three.\nDel")).toEqual([
      "Gamma three.",
    ]);
    expect(chunker.flush()).toEqual(["Del"]);
  });

  test("never splits inside a number or a URL", () => {
    expect(run(["The score is 3.5 out of 4.0 today. Next."])).toEqual([
      "The score is 3.5 out of 4.0 today.",
      "Next.",
    ]);
    expect(run(["See example.com/a.b for 10:30 details. Done."])).toEqual([
      "See example.com/a.b for 10:30 details.",
      "Done.",
    ]);
    // A number arriving a token at a time is not cut at the point.
    const chunker = new SentenceChunker();
    expect(chunker.push("It costs 3.")).toEqual([]);
    expect(chunker.push("It costs 3.5 dollars. Ok")).toEqual([
      "It costs 3.5 dollars.",
    ]);
  });

  test("does not treat a list marker as a sentence", () => {
    const chunks = run(["1. First item here.\n2. Second item here.\n"]);
    expect(chunks).toEqual(["1. First item here.", "2. Second item here."]);
  });

  test("never splits inside an inline code span", () => {
    const text = "Run `cargo test. then more: stuff` now. Then stop. ";
    const chunks = run([text]);
    expect(chunks.join(" ")).toBe(text.trim());
    expect(
      chunks.every((chunk) => (chunk.match(/`/g) ?? []).length % 2 === 0),
    ).toBe(true);
    const chunker = new SentenceChunker();
    // The span is still open: nothing inside it is a boundary.
    expect(chunker.push("Run `cargo test. then more: ")).toEqual([]);
  });

  test("keeps a fenced block together", () => {
    const chunker = new SentenceChunker();
    const text = "Here:\n```\nlet a = 1. b: 2.\n```\nDone. ";
    const chunks = [...chunker.push(text), ...chunker.flush()];
    expect(chunks[0]).toBe("Here:");
    expect(chunks[1]).toBe("```\nlet a = 1. b: 2.\n```");
  });

  test("caps a chunk near 240 characters, at a space", () => {
    const long = Array.from({ length: 80 }, () => "word").join(" ");
    const chunks = run([long]);
    expect(chunks.length).toBeGreaterThan(1);
    for (const chunk of chunks)
      expect(chunk.length).toBeLessThanOrEqual(MAX_CHUNK);
    expect(chunks.join(" ")).toBe(long);
  });

  test("packs the sentences found in one push up to the cap", () => {
    const sentence = "A short sentence here. ";
    const chunker = new SentenceChunker();
    const chunks = chunker.push(sentence.repeat(30) + "x");
    expect(chunks.length).toBeGreaterThan(1);
    for (const chunk of chunks)
      expect(chunk.length).toBeLessThanOrEqual(MAX_CHUNK);
  });

  test("streaming in small pieces gives the same words as one push", () => {
    const text =
      "First, we reproduce it. Then we look at temp_data_root(). It returns 2.5 seconds late! Why? Because of the WAL.\nNext steps: fix and verify.";
    const pieces = text.match(/.{1,7}/gs)!;
    const flat = (chunks: string[]) => chunks.join(" ").replace(/\s+/g, " ");
    const words = text.replace(/\s+/g, " ");
    // Packing differs with how much arrives at once; the words never do.
    expect(flat(run(pieces))).toBe(words);
    expect(flat(run([text]))).toBe(words);
  });

  test("flush returns whatever is left and leaves nothing behind", () => {
    const chunker = new SentenceChunker();
    chunker.push("No boundary yet");
    expect(chunker.flush()).toEqual(["No boundary yet"]);
    expect(chunker.flush()).toEqual([]);
    expect(chunker.push("No boundary yet")).toEqual([]);
  });

  test("ignores text that shrank instead of repeating it", () => {
    const chunker = new SentenceChunker();
    chunker.push("One. Two. Three");
    expect(chunker.push("One")).toEqual([]);
  });
});
