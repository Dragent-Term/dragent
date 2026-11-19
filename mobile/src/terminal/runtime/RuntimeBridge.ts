import { prepareSessionText } from '../../transport/frames';
import { TerminalRuntimeStore } from './TerminalRuntimeStore';
import { BRIDGE_ACK_TIMEOUT_MS, COPY_LIMIT_CHARS, openableLink, RENDERER_ERROR_MESSAGE, RUNTIME_LIMITS, sanitizeKeyboardAvoidanceMetrics, validGrid, type RuntimeCallbacks, type RuntimeEnvironment, type RuntimePacket } from './types';

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);

/** One in-flight update; later snapshots coalesce until the local renderer acknowledges it. */
export class RuntimeBridge {
  private ready = false;
  private disposed = false;
  private dirty = true;
  private sequence = 0;
  private epoch = -1;
  private rawSeq = 0;
  private pending: { seq: number; epoch: number; rawSeq: number } | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    readonly id: string,
    private readonly store: TerminalRuntimeStore,
    private readonly environment: () => RuntimeEnvironment,
    private readonly callbacks: RuntimeCallbacks,
  ) {}
  start = (): void => { if (!this.disposed) this.deadline(); };
  invalidate = (): void => { if (!this.disposed) { this.dirty = true; this.flush(); } };
  receive = (raw: unknown): void => {
    if (this.disposed || typeof raw !== 'string' || raw.length === 0 || raw.length > RUNTIME_LIMITS.inputBytes * 2 + 2048) return;
    let value: unknown;
    try { value = JSON.parse(raw); } catch { return; }
    // The isolated document is the only sender: version, id and shape must match.
    if (!record(value) || value.v !== 1 || value.id !== this.id || typeof value.type !== 'string') return;
    if (value.type === 'ready') {
      if (this.ready) return;
      this.clearTimer(); this.ready = true; this.epoch = -1; this.rawSeq = 0; this.invalidate(); return;
    }
    if (value.type === 'error') {
      if (!this.ready || value.epoch === this.store.getSnapshot().epoch) this.fail();
      return;
    }
    if (!this.ready || value.epoch !== this.store.getSnapshot().epoch) return;
    if (value.type === 'ack') {
      if (!this.pending || value.seq !== this.pending.seq || value.epoch !== this.pending.epoch) return;
      this.clearTimer(); this.rawSeq = this.pending.rawSeq; this.pending = null; this.flush(); return;
    }
    const env = this.environment();
    // Renderer telemetry for keyboard avoidance: accepted whenever the
    // document is in sync, even while read-only, since the lift math needs
    // it before the next keystroke, not only while writing is allowed.
    if (value.type === 'metrics') {
      const metrics = sanitizeKeyboardAvoidanceMetrics(value);
      if (metrics) this.callbacks.onMetrics?.(metrics);
      return;
    }
    // The person's own gestures on what is already on screen: they need no
    // write access or connection, and touch nothing on the desktop.
    if (value.type === 'copy') {
      if (typeof value.text === 'string' && value.text.length > 0 && value.text.length <= COPY_LIMIT_CHARS) this.callbacks.onCopy?.(value.text);
      return;
    }
    if (value.type === 'link') {
      const url = openableLink(value.url);
      if (url) this.callbacks.onLink?.(url);
      return;
    }
    if (!env.connected) return;
    // Mirror the renderer's usable(): typed input is meaningless while the
    // resync overlay hides the grid, so forged input during resync is dropped.
    if (value.type === 'input' && env.canWrite && this.store.getSnapshot().presentation !== 'resync') {
      const input = prepareSessionText('input', value.text);
      if (input.ok) this.callbacks.onInput(input.text);
    } else if (value.type === 'resize' && env.canResize && validGrid(value.rows, value.cols)) {
      const rows = value.rows as number, cols = value.cols as number;
      if (this.callbacks.onResize(rows, cols)) this.store.setViewport(rows, cols);
    }
  };
  /**
   * Keyboard.dismiss() cannot reach an input inside the document. Before the
   * renderer is ready nothing in it can hold focus, so there is nothing to send;
   * a failed send is not a renderer fault, as the caller only wants a keyboard gone.
   */
  blurInput = (): void => {
    if (this.disposed || !this.ready) return;
    try { this.callbacks.send({ v: 1, id: this.id, type: 'blur' }); } catch { /* Nothing to blur without a document. */ }
  };
  dispose = (): void => { this.disposed = true; this.ready = false; this.pending = null; this.clearTimer(); };
  private flush(): void {
    if (this.disposed || !this.ready || !this.dirty) return;
    const epoch = this.store.getSnapshot().epoch;
    if (epoch !== this.epoch) {
      this.epoch = epoch; this.rawSeq = 0; this.pending = null; this.clearTimer();
    }
    if (this.pending) return;
    const frame = this.store.frameSince(this.rawSeq);
    const packet: RuntimePacket = { v: 1, id: this.id, type: 'state', epoch, seq: ++this.sequence, frame, environment: this.environment() };
    let encoded = 0;
    try { encoded = JSON.stringify(packet).length; } catch { this.fail(); return; }
    if (encoded > RUNTIME_LIMITS.bridgeCharacters) { this.fail(); return; }
    this.dirty = false;
    this.pending = { seq: packet.seq, epoch, rawSeq: frame.lastRawSeq };
    this.deadline();
    try { this.callbacks.send(packet); } catch { this.fail(); }
  }
  private deadline(): void {
    if (this.disposed) return;
    this.clearTimer();
    this.timer = setTimeout(() => this.fail(), BRIDGE_ACK_TIMEOUT_MS);
  }
  private clearTimer(): void { if (this.timer !== null) clearTimeout(this.timer); this.timer = null; }
  private fail(): void {
    if (this.disposed) return;
    this.ready = false; this.pending = null; this.clearTimer();
    this.callbacks.onError?.(RENDERER_ERROR_MESSAGE);
  }
}
