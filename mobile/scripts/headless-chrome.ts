/**
 * Headless Chrome over the DevTools protocol, for the terminal renderer's
 * bench and pixel-parity test, plus a small PNG decoder for its screenshots.
 * No dependencies beyond Node and an installed Chrome.
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import { crc32, deflateSync, inflateSync } from 'node:zlib';

export const CHROME = process.env.CHROME_PATH ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
export const chromeAvailable = (): boolean => existsSync(CHROME);

export type Json = Record<string, unknown>;

export class Cdp {
  private next = 0;
  private readonly waiting = new Map<number, { resolve: (value: Json) => void; reject: (error: Error) => void }>();
  private readonly listeners = new Map<string, Array<(params: Json) => void>>();
  private constructor(private readonly socket: WebSocket) {
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(String(event.data)) as { id?: number; result?: Json; error?: { message: string }; method?: string; params?: Json };
      if (message.id !== undefined) {
        const entry = this.waiting.get(message.id);
        this.waiting.delete(message.id);
        if (message.error) entry?.reject(new Error(message.error.message)); else entry?.resolve(message.result ?? {});
      } else if (message.method) for (const listener of this.listeners.get(message.method) ?? []) listener(message.params ?? {});
    });
  }
  static async connect(url: string): Promise<Cdp> {
    const socket = new WebSocket(url);
    await new Promise((resolve, reject) => { socket.addEventListener('open', resolve); socket.addEventListener('error', reject); });
    return new Cdp(socket);
  }
  send(method: string, params: Json = {}, sessionId?: string): Promise<Json> {
    const id = ++this.next;
    this.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    return new Promise((resolve, reject) => this.waiting.set(id, { resolve, reject }));
  }
  on(method: string, listener: (params: Json) => void): void {
    this.listeners.set(method, [...(this.listeners.get(method) ?? []), listener]);
  }
  close(): void { this.socket.close(); }
}

/** Start headless Chrome with `flags` and connect to it. */
export async function launchChrome(profile: string, flags: string[]): Promise<{ cdp: Cdp; chrome: ChildProcess; close: () => Promise<void> }> {
  const chrome = spawn(CHROME, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--hide-scrollbars', ...flags, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
  const endpoint = await new Promise<string>((resolve, reject) => {
    let buffer = '';
    chrome.stderr?.on('data', (chunk: Buffer) => {
      buffer += chunk.toString();
      const match = /DevTools listening on (ws:\/\/\S+)/u.exec(buffer);
      if (match?.[1]) resolve(match[1]);
    });
    chrome.on('exit', () => reject(new Error('Chrome exited before DevTools was ready')));
  });
  const cdp = await Cdp.connect(endpoint);
  return {
    cdp, chrome,
    close: async () => {
      cdp.close();
      const exited = new Promise((resolve) => chrome.once('exit', resolve));
      chrome.kill();
      await exited;
    },
  };
}

/** A page target with helpers to send it commands and evaluate expressions. */
export async function openPage(cdp: Cdp): Promise<{ targetId: string; send: (method: string, params?: Json) => Promise<Json>; evaluate: (expression: string) => Promise<unknown>; close: () => Promise<void> }> {
  const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank' }) as { targetId: string };
  const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true }) as { sessionId: string };
  const send = (method: string, params: Json = {}) => cdp.send(method, params, sessionId);
  const evaluate = async (expression: string): Promise<unknown> => {
    const reply = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }) as { result: { value?: unknown }; exceptionDetails?: { text: string; exception?: { description?: string } } };
    if (reply.exceptionDetails) throw new Error(reply.exceptionDetails.exception?.description ?? reply.exceptionDetails.text);
    return reply.result.value;
  };
  return { targetId, send, evaluate, close: async () => { await cdp.send('Target.closeTarget', { targetId }); } };
}

export interface Image { width: number; height: number; rgba: Uint8Array }

/** Decode an 8-bit, non-interlaced RGB or RGBA PNG (what Chrome's screenshots are). */
export function decodePng(png: Buffer): Image {
  if (png.readUInt32BE(0) !== 0x89504e47) throw new Error('not a PNG');
  let offset = 8, width = 0, height = 0, channels = 0;
  const data: Buffer[] = [];
  while (offset < png.length) {
    const length = png.readUInt32BE(offset), type = png.toString('latin1', offset + 4, offset + 8);
    const body = png.subarray(offset + 8, offset + 8 + length);
    if (type === 'IHDR') {
      width = body.readUInt32BE(0); height = body.readUInt32BE(4);
      const depth = body[8], color = body[9], interlace = body[12];
      if (depth !== 8 || interlace !== 0 || (color !== 2 && color !== 6)) throw new Error(`unsupported PNG (depth ${depth}, colour ${color}, interlace ${interlace})`);
      channels = color === 6 ? 4 : 3;
    } else if (type === 'IDAT') data.push(body);
    else if (type === 'IEND') break;
    offset += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(data)), stride = width * channels;
  const pixels = new Uint8Array(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)] ?? 0, line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? pixels[y * stride + x - channels] ?? 0 : 0;
      const up = y > 0 ? pixels[(y - 1) * stride + x] ?? 0 : 0;
      const corner = x >= channels && y > 0 ? pixels[(y - 1) * stride + x - channels] ?? 0 : 0;
      const value = line[x] ?? 0;
      let out: number;
      if (filter === 0) out = value;
      else if (filter === 1) out = value + left;
      else if (filter === 2) out = value + up;
      else if (filter === 3) out = value + ((left + up) >> 1);
      else {
        const p = left + up - corner, pa = Math.abs(p - left), pb = Math.abs(p - up), pc = Math.abs(p - corner);
        out = value + (pa <= pb && pa <= pc ? left : pb <= pc ? up : corner);
      }
      pixels[y * stride + x] = out & 255;
    }
  }
  if (channels === 4) return { width, height, rgba: pixels };
  const rgba = new Uint8Array(width * height * 4);
  for (let index = 0; index < width * height; index++) {
    rgba[index * 4] = pixels[index * 3] ?? 0; rgba[index * 4 + 1] = pixels[index * 3 + 1] ?? 0; rgba[index * 4 + 2] = pixels[index * 3 + 2] ?? 0; rgba[index * 4 + 3] = 255;
  }
  return { width, height, rgba };
}

/** Encode RGBA pixels as a PNG (for diff images). */
export function encodePng(image: Image): Buffer {
  const chunk = (type: string, body: Buffer) => {
    const head = Buffer.alloc(8); head.writeUInt32BE(body.length, 0); head.write(type, 4, 'latin1');
    const tail = Buffer.alloc(4); tail.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), body])) >>> 0, 0);
    return Buffer.concat([head, body, tail]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(image.width, 0); header.writeUInt32BE(image.height, 4); header[8] = 8; header[9] = 6;
  const stride = image.width * 4, raw = Buffer.alloc((stride + 1) * image.height);
  for (let y = 0; y < image.height; y++) raw.set(image.rgba.subarray(y * stride, (y + 1) * stride), y * (stride + 1) + 1);
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}
