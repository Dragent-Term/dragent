import * as SecureStore from "expo-secure-store";
import type { KeyValueStore } from "./types";

// There is intentionally no secure.web.ts: end-to-end encryption refuses to
// run on the web platform (docs/design/e2ee.md §1.3), so credentials have no
// development/preview store. Importing this module on the web fails closed.

const options: SecureStore.SecureStoreOptions = {
  keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY,
};

export const secureStorage: KeyValueStore = {
  getItem: (key) => SecureStore.getItemAsync(key, options),
  setItem: (key, value) => SecureStore.setItemAsync(key, value, options),
  removeItem: (key) => SecureStore.deleteItemAsync(key, options),
};
