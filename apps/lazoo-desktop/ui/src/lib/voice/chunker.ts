// Turns a growing reply into speakable chunks. `push(fullTextSoFar)` returns
// only the chunks that cannot change any more; `flush()` returns the rest once
// the turn is over.
//
// A boundary is `.`, `!`, `?` or `:` followed by whitespace, or a newline.
// Never inside a number (3.5), a URL, or a backtick code span. Sentences found
// in one push are packed into chunks of at most MAX_CHUNK characters.

export const MAX_CHUNK = 240;

interface Segment {
  text: string;
  /** Ended by a newline or a forced split: nothing is packed after it. */
  hard: boolean;
}

const CLOSERS = new Set(['"', "'", ")", "]", "*", "_", "”", "’"]);
const SENTENCE_END = new Set([".", "!", "?", ":", "…"]);

export class SentenceChunker {
  private text = "";
  private consumed = 0;

  push(fullText: string): string[] {
    if (fullText.length < this.consumed) return [];
    this.text = fullText;
    return pack(this.scan(false));
  }

  flush(): string[] {
    return pack(this.scan(true));
  }

  reset() {
    this.text = "";
    this.consumed = 0;
  }

  private scan(final: boolean): Segment[] {
    const text = this.text;
    const segments: Segment[] = [];
    let start = this.consumed;
    let index = start;
    let lastSpace = -1;
    let fence = 0; // length of the backtick run that opened the current span

    const emit = (end: number, hard: boolean) => {
      const piece = text.slice(start, end).trim();
      if (piece) segments.push({ text: piece, hard });
      // A newline right after a sentence end closes that sentence's chunk.
      else if (hard && segments.length)
        segments[segments.length - 1]!.hard = true;
      start = end;
      index = end;
      lastSpace = -1;
      fence = 0;
    };

    while (index < text.length) {
      if (index - start >= MAX_CHUNK) {
        emit(lastSpace > start ? lastSpace : index, true);
        continue;
      }
      const char = text[index]!;
      if (char === "`") {
        let run = 1;
        while (text[index + run] === "`") run += 1;
        if (fence === 0) fence = run;
        else if (run === fence) fence = 0;
        index += run;
        continue;
      }
      if (/\s/.test(char)) lastSpace = index;
      if (fence > 0) {
        index += 1;
        continue;
      }
      if (char === "\n") {
        emit(index + 1, true);
        continue;
      }
      if (SENTENCE_END.has(char)) {
        let end = index + 1;
        while (end < text.length && SENTENCE_END.has(text[end]!)) end += 1;
        while (end < text.length && CLOSERS.has(text[end]!)) end += 1;
        if (end >= text.length) {
          // The next character decides (3. then 5), so wait for it.
          if (!final) break;
          index = end;
          continue;
        }
        if (/\s/.test(text[end]!) && !isListMarker(text.slice(start, end))) {
          emit(end, false);
          continue;
        }
        index = end;
        continue;
      }
      index += 1;
    }

    if (final) {
      // Whatever is left: split it if it is long, never leave text behind.
      while (text.length - start > MAX_CHUNK) {
        const cut = text.lastIndexOf(" ", start + MAX_CHUNK);
        emit(cut > start ? cut : start + MAX_CHUNK, true);
      }
      emit(text.length, true);
    }
    this.consumed = start;
    return segments;
  }
}

/** "1." or "2)" at the start of a line is a list marker, not a sentence. */
function isListMarker(piece: string): boolean {
  return /^\s*\d+[.)]\s*$/.test(piece);
}

function pack(segments: Segment[]): string[] {
  const chunks: string[] = [];
  let current = "";
  let closed = false;
  for (const segment of segments) {
    if (
      current &&
      !closed &&
      current.length + 1 + segment.text.length <= MAX_CHUNK
    ) {
      current = `${current} ${segment.text}`;
    } else {
      if (current) chunks.push(current);
      current = segment.text;
    }
    closed = segment.hard;
  }
  if (current) chunks.push(current);
  return chunks;
}
