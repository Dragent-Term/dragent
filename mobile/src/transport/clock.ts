/** Shared injectable clock for transport clients. No imports: every transport module may use it. */
export interface RealtimeClock {
  now(): number;
  setTimeout(callback: () => void, delayMs: number): unknown;
  clearTimeout(handle: unknown): void;
}
