/**
 * Turns a phone draft into what a terminal program needs to receive it as one
 * message: the text first (bracketed when the program asked for that), then,
 * after a gap, Enter on its own. Agent UIs handle a paste asynchronously, so a
 * CR written in the same burst is taken into the paste and never submits.
 */
import { utf8ByteLength } from "../../transport/frames";
import { COMPANION_TERMINAL_LIMITS } from "../../transport/relayRpcMethods";
import { COMPANION_TERMINAL_MESSAGES } from "./types";

/** Long enough for every agent the desktop knows, so the phone need not detect which one runs. */
export const SUBMIT_CR_DELAY_MS = 160;
export const SUBMIT_ENTER = "\r";

const PASTE_START = "\u001b[200~";
const PASTE_END = "\u001b[201~";

export const DRAFT_EMPTY_MESSAGE = "There is no text to send.";
export const DRAFT_TOO_LARGE_MESSAGE = "Input is too large. Shorten it and try again.";

export type DeliveryResult = { ok: true } | { ok: false; message: string };

/** Same result as the desktop: outer whitespace goes, every line ending becomes CR. */
export function normalizeDraft(draft: string): string {
  return draft.trim().replace(/\r\n?|\n/g, "\r");
}

/** Both CSI introducers go, so no paste-end sequence can survive or re-form. */
function withoutEscapes(text: string): string {
  // oxlint-disable-next-line no-control-regex -- Removing these control bytes is the point.
  return text.replace(/[\u001b\u009b]/g, "");
}

/**
 * The bytes of the text write. Without bracketed paste there is no way to mark
 * a line ending as data, so CRs go out as they do on the desktop.
 */
export function encodeDraft(draft: string, modes: { readonly bracketedPaste: boolean }): string {
  const text = normalizeDraft(draft);
  return modes.bracketedPaste ? PASTE_START + withoutEscapes(text) + PASTE_END : text;
}

/** Why a draft cannot go in any paste mode; decided before it takes a queue place. */
export function draftRefusal(draft: string): string | null {
  const text = normalizeDraft(draft);
  if (withoutEscapes(text).trim() === "") return DRAFT_EMPTY_MESSAGE;
  const largest = utf8ByteLength(PASTE_START + text + PASTE_END);
  return largest > COMPANION_TERMINAL_LIMITS.inputUtf8Bytes ? DRAFT_TOO_LARGE_MESSAGE : null;
}

/**
 * What stays in the field once `sent` is delivered. The field stays editable
 * while sending, so text typed after the sent part is kept; an edit inside it
 * means the person changed their mind, so nothing is removed.
 */
export function draftAfterDelivery(current: string, sent: string): string {
  return current.startsWith(sent) ? current.slice(sent.length) : current;
}

/** One Send press. A press while one is in flight does nothing: the guard is a ref, not a disabled field. */
export async function sendDraftOnce(
  guard: { current: boolean },
  submit: () => Promise<DeliveryResult>,
  ui: { onStart(): void; onDone(result: DeliveryResult): void },
): Promise<void> {
  if (guard.current) return;
  guard.current = true;
  ui.onStart();
  let result: DeliveryResult;
  try {
    result = await submit();
  } catch {
    result = { ok: false, message: COMPANION_TERMINAL_MESSAGES.sendUnconfirmed };
  }
  guard.current = false;
  ui.onDone(result);
}
