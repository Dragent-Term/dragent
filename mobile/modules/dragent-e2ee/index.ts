// The phone's end-to-end encryption module (docs/design/e2ee.md §3.2, E4):
// dragent_e2ee (Rust) through uniffi, reached over JSI. There is no
// JavaScript cryptography and no fallback; on the web it refuses to load.
import { requireOptionalNativeModule } from 'expo';
import { Platform } from 'react-native';

import { resolveE2ee } from './src/api';
import type { NativeE2ee } from './src/types';

export * from './src/api';
export type * from './src/types';

let cached: NativeE2ee | null = null;

/** The native E2EE module. Throws E2EE_UNSUPPORTED_PLATFORM on the web. */
export function getE2ee(): NativeE2ee {
  cached ??= resolveE2ee(Platform.OS, () => requireOptionalNativeModule<NativeE2ee>('DragentE2EE'));
  return cached;
}
