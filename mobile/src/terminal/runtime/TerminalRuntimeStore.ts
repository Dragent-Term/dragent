import type { TerminalDelta } from '../../transport/protocol';
import { normalizeLine, unsigned } from '../normalize';
import type { TerminalAltScreen } from '../types';
import { GRID_RESYNC_MESSAGE, RAW_RESYNC_MESSAGE, RUNTIME_LIMITS, validGrid, type RawChunk, type RuntimeFrame, type RuntimeScreenModes, type RuntimeSnapshot } from './types';

const decodedBytes = (data: string): number => data.length / 4 * 3 - (data.endsWith('==') ? 2 : data.endsWith('=') ? 1 : 0);
const empty = (epoch: number, revision: number): RuntimeSnapshot => Object.freeze({ epoch, revision, presentation: 'native', reason: null });
export const EMPTY_RUNTIME_SNAPSHOT: RuntimeSnapshot = empty(0, 0);

const EMPTY_SCREEN_MODES: RuntimeScreenModes = Object.freeze({
  alternateScreen: false,
  mouseReporting: false,
  sgrMouse: false,
  bracketedPaste: false,
});

/**
 * Raw bytes belong to one connection epoch. Styled host snapshots remain authoritative.
 *
 * Raw history is a rolling window (`RUNTIME_LIMITS.rawBytes` / `rawChunks`):
 * once over either limit the oldest chunks go, and the stream keeps rendering.
 * A renderer that already holds the older bytes is unaffected. One that needs
 * bytes no longer held (a restarted document asking from 0, or one that fell
 * behind the window) gets no chunks at all — never partial VT — and the
 * replay-unavailable listeners hear about it once per epoch, so the source can
 * redraw the screen from a seed in a fresh epoch.
 */
export class TerminalRuntimeStore {
  private snapshot = EMPTY_RUNTIME_SNAPSHOT;
  private listeners = new Set<() => void>();
  private chunks: RawChunk[] = [];
  private bytes = 0;
  private lastSeq = 0;
  private trimmed = false;
  private replayGapSignalled = false;
  private replayListeners = new Set<() => void>();
  private styled = false;
  private rawValid = true;
  private grid: TerminalAltScreen | null = null;
  private rows = 24;
  private cols = 80;
  private screenModes: RuntimeScreenModes = EMPTY_SCREEN_MODES;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;

  getSnapshot = (): RuntimeSnapshot => this.snapshot;
  subscribe = (listener: () => void): (() => void) => {
    if (this.disposed) return () => {};
    this.listeners.add(listener); return () => this.listeners.delete(listener);
  };
  reset = (): ((delta: TerminalDelta) => void) => {
    if (this.disposed) return () => {};
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.chunks = []; this.bytes = 0; this.lastSeq = 0; this.styled = false; this.rawValid = true; this.grid = null;
    this.trimmed = false; this.replayGapSignalled = false;
    this.rows = 24; this.cols = 80;
    this.screenModes = EMPTY_SCREEN_MODES;
    const epoch = this.snapshot.epoch + 1;
    this.snapshot = empty(epoch, this.snapshot.revision + 1); this.notify();
    return (delta) => { if (!this.disposed && this.snapshot.epoch === epoch) this.receive(delta); };
  };
  frameSince = (rawSeq: number): RuntimeFrame => {
    const oldest = this.chunks[0];
    const missing = this.rawValid && this.trimmed && oldest !== undefined && rawSeq < oldest.seq - 1;
    if (missing) this.signalReplayGap();
    return {
      ...this.snapshot,
      rawReset: rawSeq === 0,
      // Bytes this renderer needs are gone: send none and keep its cursor where
      // it is, so nothing after the hole is ever written. A seed epoch follows.
      rawChunks: !this.rawValid || missing ? [] : this.chunks.filter((chunk) => chunk.seq > rawSeq),
      lastRawSeq: missing ? rawSeq : this.lastSeq, rows: this.rows, cols: this.cols, grid: this.grid,
      screenModes: this.screenModes,
    };
  };
  /** Told (once per epoch, asynchronously) when a renderer needs raw bytes the window dropped. */
  subscribeReplayUnavailable = (listener: () => void): (() => void) => {
    if (this.disposed) return () => {};
    this.replayListeners.add(listener); return () => { this.replayListeners.delete(listener); };
  };
  /** True while the window no longer starts at the epoch's first chunk. */
  isRawTrimmed = (): boolean => this.trimmed;
  setViewport = (rows: number, cols: number): void => {
    if (this.disposed || !validGrid(rows, cols) || (rows === this.rows && cols === this.cols)) return;
    this.rows = rows; this.cols = cols; this.change();
  };
  setScreenModes = (modes: RuntimeScreenModes): void => {
    if (this.disposed) return;
    const next = Object.freeze({
      alternateScreen: modes.alternateScreen === true,
      mouseReporting: modes.mouseReporting === true,
      sgrMouse: modes.sgrMouse === true,
      bracketedPaste: modes.bracketedPaste === true,
    });
    if (
      next.alternateScreen === this.screenModes.alternateScreen &&
      next.mouseReporting === this.screenModes.mouseReporting &&
      next.sgrMouse === this.screenModes.sgrMouse &&
      next.bracketedPaste === this.screenModes.bracketedPaste
    ) {
      return;
    }
    this.screenModes = next;
    this.change();
  };
  dispose = (): void => {
    this.disposed = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null; this.chunks = []; this.grid = null; this.listeners.clear(); this.replayListeners.clear();
    this.snapshot = empty(this.snapshot.epoch + 1, this.snapshot.revision + 1);
  };
  private receive(delta: TerminalDelta): void {
    if (delta.kind === 'block_lines') {
      this.styled = true; this.chunks = []; this.bytes = 0;
      this.change(this.grid ? 'alternate' : 'native');
    } else if (delta.kind === 'alt_screen') {
      if (!validGrid(delta.rows, delta.cols) || !Array.isArray(delta.lines)) {
        // The host is in full-screen mode, so later raw PTY bytes must stay
        // suppressed until the epoch resets or a styled snapshot recovers.
        this.styled = true; this.chunks = []; this.bytes = 0;
        this.grid = null; this.change('resync', GRID_RESYNC_MESSAGE); return;
      }
      const rows = delta.rows as number, cols = delta.cols as number;
      // Slice before normalizing so a hostile host cannot force unbounded work.
      const normalized = delta.lines.slice(0, rows)
        .map((line, index) => normalizeLine(line, index))
        .filter((line) => line !== null);
      this.grid = Object.freeze({ rows, columns: cols,
        cursorRow: unsigned(delta.cursor_row) ? Math.min(rows - 1, delta.cursor_row) : 0,
        cursorColumn: unsigned(delta.cursor_col) ? Math.min(cols - 1, delta.cursor_col) : 0,
        cursorVisible: delta.cursor_visible === true,
        lines: normalized,
        truncated: delta.lines.length > rows || normalized.length < Math.min(rows, delta.lines.length)
          || normalized.some((line) => line.truncated),
      });
      this.styled = true; this.chunks = []; this.bytes = 0; this.rows = rows; this.cols = cols;
      this.change('alternate', null);
    } else if (delta.kind === 'alt_screen_ended') {
      this.grid = null; this.change(this.styled ? 'native' : this.rawValid && this.chunks.length ? 'raw' : 'native', null);
    } else if (delta.kind === 'output_chunk' && !this.styled) {
      if (!unsigned(delta.seq) || delta.seq < 1 || typeof delta.data_base64 !== 'string' || !delta.data_base64) return;
      if (delta.seq <= this.lastSeq) return;
      // Once invalidated, later bytes in this epoch must not render partially.
      if (!this.rawValid) return;
      const data = delta.data_base64;
      const valid = data.length % 4 === 0 && /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(data);
      const bytes = decodedBytes(data);
      if (delta.seq !== this.lastSeq + 1 || !valid) {
        this.rawValid = false; this.chunks = []; this.bytes = 0;
        this.change('resync', RAW_RESYNC_MESSAGE); return;
      }
      this.lastSeq = delta.seq; this.bytes += bytes;
      this.chunks.push(Object.freeze({ seq: delta.seq, data }));
      // Rolling window: the newest chunk always stays, however large.
      while (this.chunks.length > 1 && (this.bytes > RUNTIME_LIMITS.rawBytes || this.chunks.length > RUNTIME_LIMITS.rawChunks)) {
        const dropped = this.chunks.shift() as RawChunk;
        this.bytes -= decodedBytes(dropped.data); this.trimmed = true;
      }
      this.change('raw');
    }
  }
  private signalReplayGap(): void {
    if (this.replayGapSignalled || this.disposed) return;
    this.replayGapSignalled = true;
    const epoch = this.snapshot.epoch;
    queueMicrotask(() => {
      if (this.disposed || this.snapshot.epoch !== epoch) return;
      for (const listener of Array.from(this.replayListeners)) {
        try { listener(); } catch { /* A listener fault never breaks rendering. */ }
      }
    });
  }
  private change(presentation = this.snapshot.presentation, reason = this.snapshot.reason): void {
    this.snapshot = Object.freeze({ epoch: this.snapshot.epoch, revision: this.snapshot.revision + 1, presentation, reason });
    if (this.timer !== null) return;
    this.timer = setTimeout(() => { this.timer = null; this.notify(); }, 16);
  }
  private notify(): void {
    const listeners = Array.from(this.listeners);
    for (const listener of listeners) listener();
  }
}

interface RuntimeSource {
  getConnectionSnapshot(): { status: string; connectionEpoch: number };
  subscribeConnection(listener: () => void): () => void;
  subscribeDelta(listener: (delta: TerminalDelta) => void): () => void;
}
/** Attach before connect, so recovery clears the raw parser before any replay. */
export function attachTerminalRuntime(store: TerminalRuntimeStore, source: RuntimeSource): () => void {
  let previous = source.getConnectionSnapshot();
  let receive = store.reset();
  const state = source.subscribeConnection(() => {
    const next = source.getConnectionSnapshot();
    // Any epoch bump means a fresh socket: drop raw bytes, grids and styled
    // flags before replay so no partial VT stream survives the reconnect.
    if (next.connectionEpoch !== previous.connectionEpoch) receive = store.reset();
    else if (next.status !== previous.status
      && ['connecting', 'reconnecting', 'idle'].includes(next.status)) receive = store.reset();
    previous = next;
  });
  const delta = source.subscribeDelta((entry) => receive(entry));
  return () => { state(); delta(); receive = () => {}; };
}
