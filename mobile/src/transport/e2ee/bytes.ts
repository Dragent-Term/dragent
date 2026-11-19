/**
 * Byte plumbing for the E2EE transport (UTF-8, hex, base64). No cryptography:
 * every seal/open runs in the native module (docs/design/e2ee.md §3.2). Pure
 * JS so it runs under Hermes and the Node tests alike.
 */

/** Strict UTF-8 encode (lone surrogates become U+FFFD, as encoders do). */
export function utf8Encode(text: string): Uint8Array {
  const out: number[] = [];
  for (const character of text) {
    let point = character.codePointAt(0)!;
    if (point >= 0xd800 && point <= 0xdfff) point = 0xfffd;
    if (point < 0x80) out.push(point);
    else if (point < 0x800) out.push(0xc0 | (point >> 6), 0x80 | (point & 63));
    else if (point < 0x10000) out.push(0xe0 | (point >> 12), 0x80 | ((point >> 6) & 63), 0x80 | (point & 63));
    else out.push(0xf0 | (point >> 18), 0x80 | ((point >> 12) & 63), 0x80 | ((point >> 6) & 63), 0x80 | (point & 63));
  }
  return Uint8Array.from(out);
}

/** Strict UTF-8 decode: null on any invalid sequence (never guesses). */
export function utf8Decode(bytes: Uint8Array): string | null {
  const parts: string[] = [];
  let chunk: number[] = [];
  for (let i = 0; i < bytes.length;) {
    const b0 = bytes[i]!;
    let point: number;
    let need: number;
    if (b0 < 0x80) { point = b0; need = 0; }
    else if (b0 >= 0xc2 && b0 <= 0xdf) { point = b0 & 0x1f; need = 1; }
    else if (b0 >= 0xe0 && b0 <= 0xef) { point = b0 & 0x0f; need = 2; }
    else if (b0 >= 0xf0 && b0 <= 0xf4) { point = b0 & 0x07; need = 3; }
    else return null;
    for (let k = 1; k <= need; k++) {
      const b = bytes[i + k];
      if (b === undefined || (b & 0xc0) !== 0x80) return null;
      point = (point << 6) | (b & 0x3f);
    }
    if ((need === 2 && point < 0x800) || (need === 3 && (point < 0x10000 || point > 0x10ffff))) return null;
    if (point >= 0xd800 && point <= 0xdfff) return null;
    chunk.push(point);
    if (chunk.length >= 4096) { parts.push(String.fromCodePoint(...chunk)); chunk = []; }
    i += need + 1;
  }
  if (chunk.length) parts.push(String.fromCodePoint(...chunk));
  return parts.join("");
}

const HEX = "0123456789abcdef";

/** Lowercase hex (the §5 channel id spelling). */
export function toHex(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += HEX[b >> 4]! + HEX[b & 15]!;
  return out;
}

export function fromHex(text: string): Uint8Array | null {
  if (text.length % 2 !== 0 || !/^[0-9a-f]*$/u.test(text)) return null;
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out;
}

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/** Standard padded base64 (today's terminal payload `data` spelling). */
export function toBase64(bytes: Uint8Array): string {
  const out: string[] = [];
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i]!;
    const b = bytes[i + 1];
    const c = bytes[i + 2];
    out.push(
      B64[a >> 2]!,
      B64[((a & 3) << 4) | ((b ?? 0) >> 4)]!,
      b === undefined ? "=" : B64[((b & 15) << 2) | ((c ?? 0) >> 6)]!,
      c === undefined ? "=" : B64[c & 63]!,
    );
  }
  return out.join("");
}

/** Strict standard base64 decode; null on anything off-alphabet or misaligned. */
export function fromBase64(text: string): Uint8Array | null {
  if (text.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/u.test(text)) return null;
  const pad = text.endsWith("==") ? 2 : text.endsWith("=") ? 1 : 0;
  const out = new Uint8Array((text.length / 4) * 3 - pad);
  let o = 0;
  for (let i = 0; i < text.length; i += 4) {
    const v = [0, 1, 2, 3].map((k) => {
      const ch = text[i + k]!;
      return ch === "=" ? 0 : B64.indexOf(ch);
    });
    const n = (v[0]! << 18) | (v[1]! << 12) | (v[2]! << 6) | v[3]!;
    if (o < out.length) out[o++] = (n >> 16) & 255;
    if (o < out.length) out[o++] = (n >> 8) & 255;
    if (o < out.length) out[o++] = n & 255;
  }
  return out;
}

export function concatBytes(parts: readonly Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
}

export function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}
