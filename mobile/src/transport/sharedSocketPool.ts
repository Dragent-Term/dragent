/**
 * One connection per desktop, shared by every screen showing it (its row on
 * the home screen and the machine's own screens). Two sockets to the same
 * desktop each opened an encrypted channel; the relay keeps only a couple
 * per phone and desktop, so they evicted each other.
 *
 * When the last screen leaves, the connection lingers briefly before
 * closing: moving from one screen to the next (old one unmounts, new one
 * mounts) reuses it instead of tearing it down and setting up a new
 * encrypted channel each time.
 */

export type InputLost = (handles: readonly number[], message: string) => void;

interface Built<C> {
  client: C;
  teardown: () => void;
}

interface Entry<C> extends Built<C> {
  refs: number;
  inputLost: Set<InputLost>;
  closing: unknown;
}

export interface SocketPoolOptions {
  /** How long an unused connection stays open. */
  lingerMs: number;
  setTimer?: (run: () => void, ms: number) => unknown;
  clearTimer?: (timer: unknown) => void;
}

export interface SocketPool<C> {
  /**
   * Join the connection for `key` (building it on first use). `release`
   * leaves it; the last one out closes it after the linger. Input-loss
   * notices reach every user's callback.
   */
  acquire(key: string, onInputLost: InputLost | undefined, build: (inputLost: InputLost) => Built<C>): { client: C; release: () => void };
  /** Connections open now (lingering ones included). */
  size(): number;
}

export function createSocketPool<C>({
  lingerMs,
  setTimer = (run, ms) => setTimeout(run, ms),
  clearTimer = (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
}: SocketPoolOptions): SocketPool<C> {
  const entries = new Map<string, Entry<C>>();
  return {
    acquire(key, onInputLost, build) {
      let entry = entries.get(key);
      if (!entry) {
        const listeners = new Set<InputLost>();
        const built = build((handles, message) => {
          for (const listener of listeners) listener(handles, message);
        });
        entry = { ...built, refs: 0, inputLost: listeners, closing: null };
        entries.set(key, entry);
      }
      const shared = entry;
      if (shared.closing !== null) {
        clearTimer(shared.closing);
        shared.closing = null;
      }
      shared.refs += 1;
      if (onInputLost) shared.inputLost.add(onInputLost);
      let released = false;
      return {
        client: shared.client,
        release: () => {
          if (released) return;
          released = true;
          if (onInputLost) shared.inputLost.delete(onInputLost);
          shared.refs -= 1;
          if (shared.refs > 0 || entries.get(key) !== shared) return;
          shared.closing = setTimer(() => {
            shared.closing = null;
            if (shared.refs > 0 || entries.get(key) !== shared) return;
            entries.delete(key);
            shared.teardown();
          }, lingerMs);
        },
      };
    },
    size: () => entries.size,
  };
}
