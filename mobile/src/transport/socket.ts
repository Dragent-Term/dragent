/** Structural socket contract, usable with React Native, Node, or a test double. */
export interface RealtimeSocket {
  readonly readyState: number;
  /** Binary frames arrive as ArrayBuffer when set to "arraybuffer". */
  binaryType?: string;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  onclose: ((event: { code: number }) => void) | null;
  /** Text frames, or binary E2EE frames (docs/design/e2ee.md §4.1). */
  send(data: string | ArrayBuffer): void;
  close(code?: number, reason?: string): void;
}

export interface SocketOptions {
  headers?: Readonly<Record<string, string>>;
}
export type SocketFactory = (url: string, options: SocketOptions) => RealtimeSocket;

/** React Native accepts upgrade headers in its third constructor argument. */
export const createNativeSocket: SocketFactory = (url, options) => {
  const NativeSocket = globalThis.WebSocket as unknown as {
    new(url: string, protocols: string[], options: SocketOptions): RealtimeSocket;
  };
  return new NativeSocket(url, [], options);
};

export function createRequestId(): string {
  if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
  // Load the native module only when needed. Node tests can inject this or use
  // Web Crypto, without importing a React Native module at test startup.
  const crypto = require("expo-crypto") as { randomUUID(): string };
  return crypto.randomUUID();
}
