/**
 * What the phone chat adds to a message, all served by the paired desktop
 * inside the encrypted channel:
 * - files from the phone, uploaded in pieces and named by their saved path
 *   (as the desktop's own "+" attach does);
 * - the tab's model menu, as the desktop shows it, and a pick from it;
 * - `@file` references to the workspace's files.
 * Pure helpers here; the native pickers and file reading live in the UI.
 */
import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../transport/relayRpcMethods";

/** One piece, before base64 (the request stays under the 64 KB limit). */
export const UPLOAD_CHUNK_BYTES = 45 * 1024;
/** Pieces in flight at once. */
export const UPLOAD_PARALLEL = 3;
export const MAX_UPLOAD_BYTES = 50 * 1024 * 1024;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function call<T>(client: RelayRpcClient, method: string, params: Record<string, unknown>, read: (raw: unknown) => T | null, timeoutMs = 30_000): Promise<T> {
  let raw: unknown;
  try {
    raw = await client.request(method, params, { timeoutMs });
  } catch (error) {
    if (error instanceof RelayRpcError) throw error;
    throw new RelayRpcError("The request failed.");
  }
  const parsed = read(raw);
  if (parsed === null) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return parsed;
}

// ── Uploads ──────────────────────────────────────────────────────────────────

/** Offsets and lengths of the pieces of a file of `size` bytes. */
export function uploadPieces(size: number, chunk = UPLOAD_CHUNK_BYTES): { offset: number; length: number }[] {
  const pieces: { offset: number; length: number }[] = [];
  for (let offset = 0; offset < size; offset += chunk) pieces.push({ offset, length: Math.min(chunk, size - offset) });
  return pieces;
}

export interface SavedUpload {
  readonly path: string;
  readonly name: string;
  readonly size: number;
}

export function beginUpload(client: RelayRpcClient, name: string, size: number): Promise<string> {
  if (!name || size <= 0 || size > MAX_UPLOAD_BYTES) return Promise.reject(new RelayRpcError("That file is too large to attach (50 MB at most)."));
  return call(client, "files.upload_begin", { name: name.slice(0, 512), size }, (raw) =>
    isRecord(raw) && typeof raw.upload === "string" && raw.upload.length <= 64 ? raw.upload : null,
  );
}

export function sendUploadPiece(client: RelayRpcClient, upload: string, offset: number, data: string): Promise<number> {
  return call(client, "files.upload_chunk", { upload, offset, data }, (raw) =>
    isRecord(raw) && typeof raw.received === "number" ? raw.received : null,
  );
}

export function endUpload(client: RelayRpcClient, upload: string): Promise<SavedUpload> {
  return call(client, "files.upload_end", { upload }, (raw) => {
    if (!isRecord(raw) || typeof raw.path !== "string" || typeof raw.name !== "string" || typeof raw.size !== "number") return null;
    return Object.freeze({ path: raw.path, name: raw.name, size: raw.size });
  });
}

/**
 * Send a file whose pieces `read(offset, length)` returns as base64, a few
 * at a time; `onProgress` gets the share sent (0–1).
 */
export async function uploadFile(
  client: RelayRpcClient,
  file: { name: string; size: number },
  read: (offset: number, length: number) => Promise<string>,
  onProgress: (share: number) => void,
  cancelled: () => boolean = () => false,
): Promise<SavedUpload> {
  const upload = await beginUpload(client, file.name, file.size);
  const pieces = uploadPieces(file.size);
  let next = 0;
  let sent = 0;
  const worker = async () => {
    while (next < pieces.length) {
      if (cancelled()) throw new RelayRpcError("Cancelled.");
      const piece = pieces[next++]!;
      const data = await read(piece.offset, piece.length);
      await sendUploadPiece(client, upload, piece.offset, data);
      sent += piece.length;
      onProgress(sent / file.size);
    }
  };
  await Promise.all(Array.from({ length: Math.min(UPLOAD_PARALLEL, pieces.length) }, worker));
  return endUpload(client, upload);
}

/** The message with each attached file named as the desktop's attach names it. */
export function withAttachments(text: string, paths: readonly string[]): string {
  if (paths.length === 0) return text;
  const named = paths.map((path) => `'${path.replace(/'/g, "'\\''")}'`).join(" ");
  return text.trim() ? `${text.trim()}\n\n${named}` : named;
}

// ── Models ───────────────────────────────────────────────────────────────────

export interface ChatModelChoice {
  readonly id: string;
  readonly label: string;
  /** "low" | "medium" | "high", or null for the model's default. */
  readonly effort: string | null;
}

export interface ChatModels {
  readonly available: boolean;
  readonly locked: boolean;
  readonly current: { readonly id: string; readonly effort: string | null } | null;
  readonly choices: readonly ChatModelChoice[];
}

/** One key per menu row (a model can appear once per effort level). */
export function modelKey(id: string, effort: string | null): string {
  return `${id}\u0000${effort ?? ""}`;
}

export function readChatModels(raw: unknown): ChatModels | null {
  if (!isRecord(raw) || typeof raw.available !== "boolean" || !Array.isArray(raw.choices) || raw.choices.length > 300) return null;
  const effortOf = (value: unknown) => (value === null || value === undefined ? null : typeof value === "string" && value.length <= 16 ? value : undefined);
  const choices: ChatModelChoice[] = [];
  for (const entry of raw.choices) {
    if (!isRecord(entry) || typeof entry.id !== "string" || typeof entry.label !== "string" || entry.id.length > 200) return null;
    const effort = effortOf(entry.effort);
    if (effort === undefined) return null;
    choices.push(Object.freeze({ id: entry.id, label: entry.label.slice(0, 120), effort }));
  }
  let current: ChatModels["current"] = null;
  if (isRecord(raw.current) && typeof raw.current.id === "string") {
    const effort = effortOf(raw.current.effort);
    if (effort === undefined) return null;
    current = Object.freeze({ id: raw.current.id, effort });
  }
  return Object.freeze({ available: raw.available, locked: raw.locked === true, current, choices: Object.freeze(choices) });
}

export function getChatModels(client: RelayRpcClient, tabId: number): Promise<ChatModels> {
  return call(client, "chat.models", { tab_id: tabId }, readChatModels, 10_000);
}

export function setChatModel(client: RelayRpcClient, tabId: number, id: string, effort: string | null): Promise<ChatModels> {
  return call(client, "chat.set_model", { tab_id: tabId, id, ...(effort ? { effort } : {}) }, readChatModels, 10_000);
}

// ── @file ────────────────────────────────────────────────────────────────────

/** The `@query` being typed at the end of `text`, or null. */
export function mentionAtEnd(text: string): { query: string; start: number } | null {
  const match = /(^|\s)@([^\s@'"]{0,120})$/.exec(text);
  if (!match) return null;
  return { query: match[2]!, start: match.index + match[1]!.length };
}

/** `text` with the `@query` at its end replaced by `@path ` (relative to `root`). */
export function completeMention(text: string, path: string, root: string | null): string {
  const mention = mentionAtEnd(text);
  if (!mention) return text;
  const prefix = root ? `${root.replace(/\/+$/, "")}/` : null;
  const relative = prefix && path.startsWith(prefix) ? path.slice(prefix.length) : path;
  return `${text.slice(0, mention.start)}@${relative} `;
}

export interface FoundFile {
  readonly name: string;
  readonly path: string;
  readonly folder: boolean;
}

/** Files in `base` (a workspace on the desktop) whose names match `query`. */
export function findWorkspaceFiles(client: RelayRpcClient, base: string, query: string): Promise<readonly FoundFile[]> {
  return call(client, "files.search", { query, base, limit: 20 }, (raw) => {
    const entries = isRecord(raw) && Array.isArray(raw.entries) ? raw.entries : null;
    if (!entries) return null;
    const found: FoundFile[] = [];
    for (const entry of entries.slice(0, 50)) {
      if (!isRecord(entry) || typeof entry.name !== "string" || typeof entry.path !== "string") continue;
      found.push(Object.freeze({ name: entry.name, path: entry.path, folder: entry.kind === "dir" || entry.kind === "directory" }));
    }
    return Object.freeze(found);
  });
}
