// Platform-independent part of the E2EE module: the web refusal (§1.3),
// coded errors and the JS-side benchmark. No React Native imports, so it runs
// under the Node tests.

import type { BenchResult, E2eeErrorCode, NativeE2ee } from './types';

export class E2eeError extends Error {
  readonly code: E2eeErrorCode;
  constructor(code: E2eeErrorCode, message: string) {
    super(message);
    this.name = 'E2eeError';
    this.code = code;
  }
}

/** The error code of anything the module threw, or null. */
export function e2eeErrorCode(error: unknown): E2eeErrorCode | null {
  if (typeof error === 'object' && error !== null && 'code' in error) {
    const code = (error as { code: unknown }).code;
    if (typeof code === 'string' && code.startsWith('E2EE_')) return code as E2eeErrorCode;
  }
  return null;
}

/**
 * The native module for this platform. End-to-end encryption runs only in
 * the signed iOS and Android apps: on the web (or anywhere the native module
 * is missing) this throws, and there is no JavaScript fallback (§1.3, §3.2).
 */
export function resolveE2ee(platformOS: string, load: () => NativeE2ee | null): NativeE2ee {
  if (platformOS !== 'ios' && platformOS !== 'android') {
    throw new E2eeError('E2EE_UNSUPPORTED_PLATFORM', 'End-to-end encryption runs only in the Dragent iOS and Android apps.');
  }
  const native = load();
  if (!native) {
    throw new E2eeError('E2EE_UNSUPPORTED', 'The Dragent encryption module is missing from this build.');
  }
  return platformOS === 'android' ? androidRecords(native) : native;
}

/**
 * Expo on Android converts a Uint8Array only as a top-level argument: one
 * nested in a record reaches Kotlin as a map and the call is rejected
 * ("DynamicFromMap cannot be cast to byte[]"). The record arguments' byte
 * fields go as number arrays there; the Kotlin records take List<Int>. iOS
 * converts nested bytes itself and gets the module unchanged.
 */
export function androidRecords(native: NativeE2ee): NativeE2ee {
  const list = (bytes: Uint8Array) => Array.from(bytes);
  const overrides: Partial<NativeE2ee> = {
    startPairing: (ids, desktopKey, secretP, deviceName) =>
      native.startPairing({ ...ids, chan: list(ids.chan) } as never, desktopKey, secretP, deviceName),
    startKk: (ids, desktopPin, caps) => native.startKk({ ...ids, chan: list(ids.chan) } as never, desktopPin, caps),
    encodeFrame: (frame) => native.encodeFrame({ ...frame, chan: list(frame.chan), body: list(frame.body) } as never),
  };
  const bound = new Map<PropertyKey, unknown>();
  return new Proxy(native, {
    get(target, key) {
      if (Object.prototype.hasOwnProperty.call(overrides, key)) return overrides[key as keyof NativeE2ee];
      const value = Reflect.get(target, key);
      if (typeof value !== 'function') return value;
      // Native functions are called on the module itself, never the proxy.
      if (!bound.has(key)) bound.set(key, value.bind(target));
      return bound.get(key);
    },
  });
}

export interface JsBenchResult extends BenchResult {
  /** Measured from JS: includes the JSI crossing. */
  fromJs: true;
}

/**
 * §11 / §12.12 on-device benchmark: native timings plus the JSI crossing,
 * measured from JS with a 64-byte buffer (one keystroke's record size).
 */
export function runBenchmarks(
  native: Pick<NativeE2ee, 'nativeBenchmarks' | 'sha256' | 'encodeRecords'>,
  iterations = 2000,
  now: () => number = () => performance.now(),
): Array<BenchResult | JsBenchResult> {
  const results: Array<BenchResult | JsBenchResult> = native.nativeBenchmarks(iterations);
  const buf = new Uint8Array(64);
  let t0 = now();
  for (let i = 0; i < iterations; i++) native.sha256(buf);
  results.push({ name: 'jsi_call_64b', iterations, microsPerOp: ((now() - t0) * 1000) / iterations, fromJs: true });
  t0 = now();
  for (let i = 0; i < iterations; i++) native.encodeRecords('json', i + 1, buf.subarray(0, 20));
  results.push({ name: 'jsi_encode_keystroke_record', iterations, microsPerOp: ((now() - t0) * 1000) / iterations, fromJs: true });
  return results;
}
