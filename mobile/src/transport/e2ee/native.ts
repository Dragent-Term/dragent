/**
 * App wiring for the phone's E2EE transport: the one PhoneSecurity of the
 * process, built on the native module (docs/design/e2ee.md §3.2). Imports
 * React Native through the module index, so tests use the pure files.
 */
import { e2eeErrorCode, getE2ee } from "../../../modules/dragent-e2ee";
import type { RelaySocketE2ee } from "../RelaySocketClient";
import { PhoneSecurity, type UnpairOptions, type UnpairResult } from "./security";
import { forgetPins } from "../../machines/pins";

let security: PhoneSecurity | null = null;

/** The process's PhoneSecurity. Throws E2EE_UNSUPPORTED_PLATFORM on the web (§1.3). */
export function phoneSecurity(): PhoneSecurity {
  security ??= new PhoneSecurity(getE2ee());
  return security;
}

/**
 * The desktop's name from pairing (`pair-done`), or null. After the E2EE
 * cut-over the server knows no names, so screens must use this rather than
 * falling back to the desktop's id.
 */
export function pairedDesktopName(desktopId: string): string | null {
  try {
    return phoneSecurity().desktopName(desktopId);
  } catch {
    return null;
  }
}

export const E2EE_WEB_MESSAGE = "End-to-end encryption runs only in the Dragent iOS and Android apps.";
export const E2EE_MISSING_MESSAGE = "This build of Dragent is missing its encryption module. Update the app.";
export const E2EE_NOT_PAIRED_MESSAGE = "Pair this phone again to keep using it. Phone access is now end-to-end encrypted.";

/** E2EE config for a targeted desktop socket; `unavailable` fails closed. */
export function socketE2ee(account: string | null | undefined, phone: string | null | undefined): RelaySocketE2ee {
  if (!account || !phone) return { unavailable: E2EE_NOT_PAIRED_MESSAGE };
  try {
    return { security: phoneSecurity(), account, phone };
  } catch (error) {
    return { unavailable: e2eeErrorCode(error) === "E2EE_UNSUPPORTED_PLATFORM" ? E2EE_WEB_MESSAGE : E2EE_MISSING_MESSAGE };
  }
}

/**
 * §6.4 "A phone unpairs a desktop": local, total and immediate, then the
 * server (availability only). Used by Settings → Remove desktop (E5c).
 */
export async function unpairDesktop(desktopId: string, options?: UnpairOptions): Promise<UnpairResult> {
  const result = await phoneSecurity().unpairDesktop(desktopId, options);
  // Its pinned workspaces mean nothing once it's unpaired.
  await forgetPins(desktopId).catch(() => undefined);
  return result;
}
