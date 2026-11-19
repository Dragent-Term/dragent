import assert from 'node:assert/strict';
import { test } from 'node:test';
import { RuntimeBridge } from '../src/terminal/runtime/RuntimeBridge';
import { TerminalRuntimeStore, attachTerminalRuntime } from '../src/terminal/runtime/TerminalRuntimeStore';
import {
  GRID_RESYNC_MESSAGE,
  RAW_RESYNC_MESSAGE,
  RUNTIME_LIMITS,
  validGrid,
  type RuntimeBlurPacket,
  type RuntimeEnvironment,
  type RuntimePacket,
} from '../src/terminal/runtime/types';
import type { TerminalDelta } from '../src/transport/wire-types.generated';
import type { TerminalPalette } from '../src/theme/types';

const palette: TerminalPalette = {
  background: '#101010',
  foreground: '#eeeeee',
  cursor: '#abcdef',
  ansiNormal: ['#000000', '#cc0000', '#00cc00', '#cccc00', '#0000cc', '#cc00cc', '#00cccc', '#cccccc'],
  ansiBright: ['#555555', '#ff0000', '#00ff00', '#ffff00', '#0000ff', '#ff00ff', '#00ffff', '#ffffff'],
};
const env = (overrides: Partial<RuntimeEnvironment> = {}): RuntimeEnvironment => ({
  connected: true,
  canWrite: true,
  canResize: true,
  holdFit: false,
  palette,
  ...overrides,
});
const encoded = (text: string): string => Buffer.from(text, 'utf8').toString('base64');
const chunk = (seq: number, text = `line-${seq}`): TerminalDelta =>
  ({ kind: 'output_chunk', block_id: 'b1', seq, data_base64: encoded(text) }) as TerminalDelta;
const altScreen = (overrides: Record<string, unknown> = {}): TerminalDelta =>
  ({
    kind: 'alt_screen',
    rows: 2,
    cols: 10,
    cursor_row: 0,
    cursor_col: 1,
    cursor_visible: true,
    lines: [{ spans: [{ text: 'hi', flags: 1 }] }],
    ...overrides,
  }) as unknown as TerminalDelta;

function drive(store: TerminalRuntimeStore, deltas: TerminalDelta[]): void {
  const receive = store.reset();
  for (const delta of deltas) receive(delta);
}

// ── TerminalRuntimeStore: raw queue ──────────────────────────────────────────

test('raw bytes require a contiguous sequence from 1 and resync explicitly on gaps', () => {
  const store = new TerminalRuntimeStore();
  try {
    drive(store, [chunk(1, 'a'), chunk(2, 'b')]);
    assert.equal(store.getSnapshot().presentation, 'raw');
    assert.equal(store.frameSince(0).rawChunks.length, 2);
    assert.equal(store.frameSince(0).rawReset, true);
    assert.equal(store.frameSince(1).rawChunks.length, 1);
    assert.equal(store.frameSince(1).rawReset, false);
    const receive = store.reset();
    receive(chunk(1, 'a'));
    receive(chunk(3, 'skipped-2'));
    assert.equal(store.getSnapshot().presentation, 'resync');
    assert.equal(store.getSnapshot().reason, RAW_RESYNC_MESSAGE);
    assert.deepEqual(store.frameSince(0).rawChunks, []);
    // Once invalidated, later bytes in the same epoch must not render partially.
    receive(chunk(4, 'late'));
    assert.equal(store.getSnapshot().presentation, 'resync');
    assert.deepEqual(store.frameSince(0).rawChunks, []);
  } finally {
    store.dispose();
  }
});

test('duplicate or stale raw sequences are ignored without resync', () => {
  const store = new TerminalRuntimeStore();
  try {
    const receive = store.reset();
    receive(chunk(1, 'a'));
    const revision = store.getSnapshot().revision;
    receive(chunk(1, 'duplicate'));
    assert.equal(store.getSnapshot().presentation, 'raw');
    assert.equal(store.getSnapshot().revision, revision);
    assert.equal(store.frameSince(0).rawChunks.length, 1);
  } finally {
    store.dispose();
  }
});

test('invalid base64 resyncs without partial output', () => {
  const store = new TerminalRuntimeStore();
  try {
    const receive = store.reset();
    receive({ kind: 'output_chunk', block_id: 'b1', seq: 1, data_base64: '!!!not-base64!!!' } as TerminalDelta);
    assert.equal(store.getSnapshot().presentation, 'resync');
    assert.equal(store.getSnapshot().reason, RAW_RESYNC_MESSAGE);
  } finally {
    store.dispose();
  }
});

test('raw history is a rolling window: trimming never resyncs and a live renderer keeps going', () => {
  const store = new TerminalRuntimeStore();
  try {
    const receive = store.reset();
    const total = RUNTIME_LIMITS.rawChunks + 10;
    for (let seq = 1; seq <= total; seq++) receive(chunk(seq, 'x'));
    assert.equal(store.getSnapshot().presentation, 'raw');
    assert.equal(store.getSnapshot().reason, null);
    assert.equal(store.isRawTrimmed(), true);
    // A renderer that already holds seq total-1 gets exactly the next chunk.
    const live = store.frameSince(total - 1);
    assert.deepEqual(live.rawChunks.map((c) => c.seq), [total]);
    assert.equal(live.lastRawSeq, total);
    // Byte limit: one big chunk pushes the small ones out; the newest always stays.
    const big = 'y'.repeat(RUNTIME_LIMITS.rawBytes - 10);
    receive(chunk(total + 1, big));
    receive(chunk(total + 2, 'zzzz'));
    const frame = store.frameSince(total);
    assert.deepEqual(frame.rawChunks.map((c) => c.seq), [total + 1, total + 2]);
    assert.equal(store.getSnapshot().presentation, 'raw');
  } finally {
    store.dispose();
  }
});

test('a renderer needing dropped bytes gets none and triggers one reseed signal per epoch', async () => {
  const store = new TerminalRuntimeStore();
  try {
    let signals = 0;
    store.subscribeReplayUnavailable(() => { signals += 1; });
    let receive = store.reset();
    receive(chunk(1));
    receive(chunk(2));
    // Nothing trimmed: a restarted renderer replays everything.
    assert.equal(store.frameSince(0).rawChunks.length, 2);
    await Promise.resolve();
    assert.equal(signals, 0);
    for (let seq = 3; seq <= RUNTIME_LIMITS.rawChunks + 5; seq++) receive(chunk(seq, 'x'));
    // A restarted WebView asks from 0: no partial VT, cursor does not move.
    const restarted = store.frameSince(0);
    assert.deepEqual(restarted.rawChunks, []);
    assert.equal(restarted.lastRawSeq, 0);
    assert.equal(restarted.rawReset, true);
    store.frameSince(0);
    await Promise.resolve();
    assert.equal(signals, 1, 'signalled once, asynchronously');
    assert.notEqual(store.getSnapshot().presentation, 'resync');
    // The seed arrives in a fresh epoch: the full (new) history is replayable.
    receive = store.reset();
    receive(chunk(1, 'seed'));
    assert.deepEqual(store.frameSince(0).rawChunks.map((c) => c.seq), [1]);
    for (let seq = 2; seq <= RUNTIME_LIMITS.rawChunks + 5; seq++) receive(chunk(seq, 'x'));
    store.frameSince(0);
    await Promise.resolve();
    assert.equal(signals, 2, 'a new epoch may signal again');
  } finally {
    store.dispose();
  }
});

test('a signal for an epoch that has since been reset is not delivered', async () => {
  const store = new TerminalRuntimeStore();
  try {
    let signals = 0;
    store.subscribeReplayUnavailable(() => { signals += 1; });
    const receive = store.reset();
    for (let seq = 1; seq <= RUNTIME_LIMITS.rawChunks + 2; seq++) receive(chunk(seq, 'x'));
    store.frameSince(0);
    store.reset();
    await Promise.resolve();
    assert.equal(signals, 0);
  } finally {
    store.dispose();
  }
});

test('styled snapshots take priority and suppress the raw byte stream', () => {
  const store = new TerminalRuntimeStore();
  try {
    const receive = store.reset();
    receive(chunk(1, 'raw-first'));
    assert.equal(store.getSnapshot().presentation, 'raw');
    receive({ kind: 'block_lines', block_id: 'b1', base_line: 0, lines: [] } as TerminalDelta);
    assert.equal(store.getSnapshot().presentation, 'native');
    assert.deepEqual(store.frameSince(0).rawChunks, []);
    // Raw bytes after styled output stay suppressed instead of doubling output.
    receive(chunk(2, 'raw-after-styled'));
    assert.equal(store.getSnapshot().presentation, 'native');
    assert.deepEqual(store.frameSince(0).rawChunks, []);
  } finally {
    store.dispose();
  }
});

// ── TerminalRuntimeStore: alternate grid ─────────────────────────────────────

test('valid alternate grids become authoritative and track the viewport', () => {
  const store = new TerminalRuntimeStore();
  try {
    const receive = store.reset();
    receive(altScreen());
    assert.equal(store.getSnapshot().presentation, 'alternate');
    const frame = store.frameSince(0);
    assert.equal(frame.rows, 2);
    assert.equal(frame.cols, 10);
    assert.ok(frame.grid);
    assert.equal(frame.grid?.cursorColumn, 1);
    assert.equal(frame.grid?.lines[0]?.spans[0]?.text, 'hi');
    assert.deepEqual(frame.rawChunks, []);
  } finally {
    store.dispose();
  }
});

test('oversized host grids resync and keep later raw bytes suppressed', () => {
  const store = new TerminalRuntimeStore();
  try {
    const receive = store.reset();
    receive(altScreen({ rows: 500, cols: 500, lines: [] }));
    assert.equal(store.getSnapshot().presentation, 'resync');
    assert.equal(store.getSnapshot().reason, GRID_RESYNC_MESSAGE);
    // The host is in full-screen mode: raw PTY bytes must not replace resync.
    receive(chunk(1, 'raw-during-alt'));
    assert.equal(store.getSnapshot().presentation, 'resync');
    assert.deepEqual(store.frameSince(0).rawChunks, []);
    // Leaving full-screen recovers to styled history, not to raw bytes.
    receive({ kind: 'alt_screen_ended' } as TerminalDelta);
    assert.equal(store.getSnapshot().presentation, 'native');
  } finally {
    store.dispose();
  }
});

test('viewport changes are bounded and epoch reset clears parser state', () => {
  const store = new TerminalRuntimeStore();
  try {
    const receive = store.reset();
    const epoch = store.getSnapshot().epoch;
    receive(chunk(1, 'a'));
    receive(altScreen());
    assert.equal(store.getSnapshot().presentation, 'alternate');
    store.setViewport(30, 100);
    assert.equal(store.frameSince(0).rows, 30);
    assert.equal(store.frameSince(0).cols, 100);
    store.setViewport(0, 0);
    assert.equal(store.frameSince(0).rows, 30);
    store.setViewport(500, 500);
    assert.equal(store.frameSince(0).rows, 30);
    const next = store.reset();
    assert.equal(store.getSnapshot().epoch, epoch + 1);
    assert.equal(store.getSnapshot().presentation, 'native');
    assert.equal(store.frameSince(0).rows, 24);
    assert.equal(store.frameSince(0).cols, 80);
    assert.deepEqual(store.frameSince(0).rawChunks, []);
    // The previous epoch's closure can no longer feed the new parser.
    receive(chunk(1, 'stale-epoch'));
    assert.equal(store.getSnapshot().presentation, 'native');
    next(chunk(1, 'fresh'));
    assert.equal(store.getSnapshot().presentation, 'raw');
  } finally {
    store.dispose();
  }
});

test('validGrid enforces the shared renderer bounds', () => {
  assert.equal(validGrid(24, 80), true);
  assert.equal(validGrid(0, 80), false);
  assert.equal(validGrid(24, 0), false);
  assert.equal(validGrid(RUNTIME_LIMITS.rows + 1, 10), false);
  assert.equal(validGrid(10, RUNTIME_LIMITS.cols + 1), false);
  assert.equal(validGrid(240, 512), false);
  assert.equal(validGrid(1.5, 80), false);
  assert.equal(validGrid('24', 80), false);
});

test('listeners can unsubscribe during notify without dropping peers', async () => {
  const store = new TerminalRuntimeStore();
  try {
    store.reset();
    let first = 0;
    let second = 0;
    let detach: (() => void) | null = null;
    detach = store.subscribe(() => {
      first++;
      detach?.();
    });
    const keep = store.subscribe(() => {
      second++;
    });
    (store as unknown as { change(p?: string, r?: string | null): void }).change('raw', null);
    await new Promise<void>((resolve) => setTimeout(resolve, 30));
    assert.equal(first, 1);
    assert.equal(second, 1);
    keep();
  } finally {
    store.dispose();
  }
});

// ── attachTerminalRuntime ────────────────────────────────────────────────────

test('attachTerminalRuntime resets before replay on epoch and reconnect status', () => {
  const store = new TerminalRuntimeStore();
  try {
    let connection = { status: 'connected', connectionEpoch: 1 };
    const connectionListeners = new Set<() => void>();
    const deltaListeners = new Set<(delta: TerminalDelta) => void>();
    const detach = attachTerminalRuntime(store, {
      getConnectionSnapshot: () => ({ ...connection }),
      subscribeConnection: (listener: () => void) => {
        connectionListeners.add(listener);
        return () => connectionListeners.delete(listener);
      },
      subscribeDelta: (listener: (delta: TerminalDelta) => void) => {
        deltaListeners.add(listener);
        return () => deltaListeners.delete(listener);
      },
    });
    try {
      const emit = (delta: TerminalDelta): void => {
        for (const listener of deltaListeners) listener(delta);
      };
      const reconnect = (next: typeof connection): void => {
        connection = next;
        for (const listener of connectionListeners) listener();
      };
      emit(chunk(1, 'first-epoch'));
      assert.equal(store.getSnapshot().presentation, 'raw');
      reconnect({ status: 'connected', connectionEpoch: 2 });
      assert.equal(store.getSnapshot().presentation, 'native');
      emit(chunk(1, 'second-epoch'));
      assert.equal(store.getSnapshot().presentation, 'raw');
      reconnect({ status: 'reconnecting', connectionEpoch: 2 });
      assert.equal(store.getSnapshot().presentation, 'native');
      assert.equal(deltaListeners.size, 1);
    } finally {
      detach();
      assert.equal(connectionListeners.size, 0);
      assert.equal(deltaListeners.size, 0);
    }
  } finally {
    store.dispose();
  }
});

// ── RuntimeBridge ────────────────────────────────────────────────────────────

interface Harness {
  bridge: RuntimeBridge;
  packets: RuntimePacket[];
  blurs: RuntimeBlurPacket[];
  inputs: string[];
  resizes: Array<{ rows: number; cols: number }>;
  metrics: Array<Record<string, unknown>>;
  errors: string[];
  environment: RuntimeEnvironment;
  sendText(type: string, extra?: Record<string, unknown>): void;
}

function harness(store: TerminalRuntimeStore, id = 'bridge-test', environment?: RuntimeEnvironment): Harness {
  const packets: RuntimePacket[] = [];
  const inputs: string[] = [];
  const resizes: Array<{ rows: number; cols: number }> = [];
  const errors: string[] = [];
  const current: RuntimeEnvironment = environment ?? env();
  const metrics: Array<Record<string, unknown>> = [];
  const blurs: RuntimeBlurPacket[] = [];
  const bridge = new RuntimeBridge(
    id,
    store,
    () => ({ ...current }),
    {
      send: (packet) => { if (packet.type === 'state') packets.push(packet); else blurs.push(packet); },
      onInput: (text) => {
        inputs.push(text);
        return true;
      },
      onResize: (rows, cols) => {
        resizes.push({ rows, cols });
        return true;
      },
      onMetrics: (reported) => metrics.push({ ...reported }),
      onError: (message) => errors.push(message),
    },
  );
  bridge.start();
  return {
    bridge,
    packets,
    blurs,
    inputs,
    resizes,
    metrics,
    errors,
    environment: current,
    sendText: (type, extra = {}) => bridge.receive(JSON.stringify({ v: 1, id, type, ...extra })),
  };
}

test('blurInput reaches a ready renderer at once, without waiting for an acknowledgement', () => {
  const store = new TerminalRuntimeStore();
  store.reset();
  const h = harness(store);
  try {
    h.sendText('ready', { epoch: -1 });
    assert.equal(h.packets.length, 1, 'a state packet is in flight, unacknowledged');
    h.bridge.blurInput();
    assert.deepEqual(h.blurs, [{ v: 1, id: 'bridge-test', type: 'blur' }]);
    assert.equal(h.packets.length, 1, 'state sequencing is untouched');
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});

test('blurInput does nothing before the renderer is ready or after it is gone', () => {
  const store = new TerminalRuntimeStore();
  store.reset();
  const h = harness(store);
  h.bridge.blurInput();
  assert.deepEqual(h.blurs, []);
  h.sendText('ready', { epoch: -1 });
  h.bridge.dispose();
  h.bridge.blurInput();
  assert.deepEqual(h.blurs, []);
  assert.deepEqual(h.errors, []);
  store.dispose();
});

test('a failed blur send is not reported as a dead renderer', () => {
  const store = new TerminalRuntimeStore();
  store.reset();
  const errors: string[] = [];
  const bridge = new RuntimeBridge('bridge-test', store, () => env(), {
    send: (packet) => { if (packet.type === 'blur') throw new Error('Renderer unavailable'); },
    onInput: () => true,
    onResize: () => true,
    onError: (message) => errors.push(message),
  });
  bridge.start();
  bridge.receive(JSON.stringify({ v: 1, id: 'bridge-test', type: 'ready', epoch: -1 }));
  assert.doesNotThrow(() => bridge.blurInput());
  assert.deepEqual(errors, []);
  bridge.dispose();
  store.dispose();
});

test('bridge handshake sends one in-flight packet and coalesces later revisions', () => {
  const store = new TerminalRuntimeStore();
  const receive = store.reset();
  const h = harness(store);
  try {
    h.sendText('ready', { epoch: -1 });
    assert.equal(h.packets.length, 1);
    assert.equal(h.packets[0]?.epoch, store.getSnapshot().epoch);
    // A store change while a packet is in flight stays dirty until the ack.
    receive(chunk(1, 'a'));
    h.bridge.invalidate();
    assert.equal(h.packets.length, 1);
    h.sendText('ack', { epoch: h.packets[0]?.epoch, seq: h.packets[0]?.seq });
    assert.equal(h.packets.length, 2);
    assert.equal(h.packets[1]?.frame.rawChunks.length, 1);
    h.sendText('ack', { epoch: h.packets[1]?.epoch, seq: h.packets[1]?.seq });
    assert.equal(h.packets.length, 2);
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});

test('bridge rejects stale documents, epochs and duplicate acknowledgements', () => {
  const store = new TerminalRuntimeStore();
  store.reset();
  const h = harness(store, 'ids-matter');
  try {
    h.bridge.receive('not-json');
    h.bridge.receive(JSON.stringify({ v: 1, id: 'other-id', type: 'ready' }));
    assert.equal(h.packets.length, 0);
    h.sendText('ready', { epoch: -1 });
    assert.equal(h.packets.length, 1);
    const first = h.packets[0]!;
    // Duplicate ready, wrong-epoch ack and wrong-seq ack are all ignored.
    h.sendText('ready', { epoch: -1 });
    h.sendText('ack', { epoch: 9999, seq: first.seq });
    h.sendText('ack', { epoch: first.epoch, seq: 9999 });
    assert.equal(h.packets.length, 1);
    h.sendText('ack', { epoch: first.epoch, seq: first.seq });
    assert.equal(h.packets.length, 1);
    // After dispose every message is dropped.
    h.bridge.dispose();
    h.sendText('ready', { epoch: -1 });
    assert.equal(h.packets.length, 1);
    assert.deepEqual(h.errors, []);
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});

test('bridge gates input and resize on connection, role and resync state', () => {
  const store = new TerminalRuntimeStore();
  const receive = store.reset();
  const h = harness(store);
  try {
    h.sendText('ready', { epoch: -1 });
    h.sendText('input', { epoch: store.getSnapshot().epoch, text: 'ls\n' });
    assert.deepEqual(h.inputs, ['ls\n']);
    h.environment.canWrite = false;
    h.sendText('input', { epoch: store.getSnapshot().epoch, text: 'blocked\n' });
    assert.deepEqual(h.inputs, ['ls\n']);
    h.environment.canWrite = true;
    h.environment.connected = false;
    h.sendText('input', { epoch: store.getSnapshot().epoch, text: 'offline\n' });
    assert.deepEqual(h.inputs, ['ls\n']);
    h.environment.connected = true;
    h.sendText('resize', { epoch: store.getSnapshot().epoch, rows: 30, cols: 100 });
    assert.deepEqual(h.resizes, [{ rows: 30, cols: 100 }]);
    h.environment.canResize = false;
    h.sendText('resize', { epoch: store.getSnapshot().epoch, rows: 31, cols: 101 });
    assert.deepEqual(h.resizes, [{ rows: 30, cols: 100 }]);
    h.environment.canResize = true;
    h.sendText('resize', { epoch: store.getSnapshot().epoch, rows: 0, cols: 0 });
    assert.deepEqual(h.resizes, [{ rows: 30, cols: 100 }]);
    // A compromised renderer cannot type while the resync overlay is visible.
    receive(chunk(1, 'a'));
    receive(chunk(3, 'gap'));
    assert.equal(store.getSnapshot().presentation, 'resync');
    h.bridge.invalidate();
    h.sendText('input', { epoch: store.getSnapshot().epoch, text: 'during-resync\n' });
    assert.deepEqual(h.inputs, ['ls\n']);
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});

test('bridge hands valid renderer metrics to the view and drops malformed ones', () => {
  const store = new TerminalRuntimeStore();
  store.reset();
  const h = harness(store);
  try {
    h.sendText('ready', { epoch: -1 });
    const epoch = store.getSnapshot().epoch;
    h.sendText('metrics', { epoch, rows: 24, cursorRow: 5, altScreen: false, rowPitch: 20, top: 0 });
    assert.deepEqual(h.metrics, [{ rows: 24, cursorRow: 5, altScreen: false, rowPitch: 20, top: 0 }]);
    // A cursor past the grid clamps instead of dropping the snapshot.
    h.sendText('metrics', { epoch, rows: 24, cursorRow: 99, altScreen: true });
    assert.deepEqual(h.metrics.at(-1), { rows: 24, cursorRow: 23, altScreen: true });
    // Telemetry is not gated on being connected, but it is on epoch and shape.
    h.environment.connected = false;
    h.sendText('metrics', { epoch, rows: 24, cursorRow: 1, altScreen: false });
    assert.equal(h.metrics.length, 3);
    h.environment.connected = true;
    h.sendText('metrics', { epoch: 9999, rows: 24, cursorRow: 1, altScreen: false });
    h.sendText('metrics', { epoch, rows: 0, cursorRow: 1, altScreen: false });
    h.sendText('metrics', { epoch, rows: 24, cursorRow: -1, altScreen: false });
    h.sendText('metrics', { epoch, rows: 24, cursorRow: 1, altScreen: 'yes' });
    h.sendText('metrics', { epoch, rows: 24, cursorRow: 1, altScreen: false, rowPitch: -4 });
    assert.equal(h.metrics.length, 3);
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});

test('bridge reports renderer errors and oversized frames as restartable failures', () => {
  const store = new TerminalRuntimeStore();
  store.reset();
  const h = harness(store);
  try {
    h.sendText('ready', { epoch: -1 });
    assert.equal(h.packets.length, 1);
    h.sendText('error', { epoch: store.getSnapshot().epoch });
    assert.equal(h.errors.length, 1);
    // After a failure the bridge waits for a fresh ready before sending again.
    const count = h.packets.length;
    h.bridge.invalidate();
    assert.equal(h.packets.length, count);
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});

test('bridge epoch changes reset the raw replay cursor', () => {
  const store = new TerminalRuntimeStore();
  let receive = store.reset();
  const h = harness(store);
  try {
    h.sendText('ready', { epoch: -1 });
    receive(chunk(1, 'epoch-one'));
    h.bridge.invalidate();
    h.sendText('ack', { epoch: h.packets.at(-1)?.epoch, seq: h.packets.at(-1)?.seq });
    assert.equal(h.packets.at(-1)?.frame.rawChunks.length, 1);
    receive = store.reset();
    receive(chunk(1, 'epoch-two'));
    h.bridge.invalidate();
    const latest = h.packets.at(-1)!;
    assert.equal(latest.frame.rawReset, true);
    assert.equal(latest.frame.rawChunks.length, 1);
    assert.equal(latest.frame.rawChunks[0]?.data, encoded('epoch-two'));
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});

test('a burst of chunks reaches the renderer as one packet', () => {
  const store = new TerminalRuntimeStore();
  const receive = store.reset();
  const h = harness(store);
  try {
    h.sendText('ready', { epoch: -1 });
    assert.equal(h.packets.length, 1);
    // A redraw burst arrives while the first packet is unacknowledged.
    for (let seq = 1; seq <= 300; seq++) receive(chunk(seq, `frame-${seq}`));
    for (let i = 0; i < 300; i++) h.bridge.invalidate();
    assert.equal(h.packets.length, 1, 'one packet in flight');
    h.sendText('ack', { epoch: h.packets[0]?.epoch, seq: h.packets[0]?.seq });
    assert.equal(h.packets.length, 2);
    assert.equal(h.packets[1]?.frame.rawChunks.length, 300, 'the whole burst in one packet');
    assert.equal(h.packets[1]?.frame.lastRawSeq, 300);
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});

test('a full raw window fits one bridge packet', () => {
  const store = new TerminalRuntimeStore();
  const receive = store.reset();
  const h = harness(store);
  try {
    const piece = 'q'.repeat(Math.floor(RUNTIME_LIMITS.rawBytes / RUNTIME_LIMITS.rawChunks));
    for (let seq = 1; seq <= RUNTIME_LIMITS.rawChunks; seq++) receive(chunk(seq, piece));
    h.sendText('ready', { epoch: -1 });
    assert.equal(h.packets.length, 1);
    assert.equal(h.packets[0]?.frame.rawChunks.length, RUNTIME_LIMITS.rawChunks);
    assert.ok(JSON.stringify(h.packets[0]).length < RUNTIME_LIMITS.bridgeCharacters);
    assert.deepEqual(h.errors, []);
  } finally {
    h.bridge.dispose();
    store.dispose();
  }
});
