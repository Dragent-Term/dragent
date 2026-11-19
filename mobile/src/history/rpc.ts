/**
 * Past sessions on the paired desktop: list a folder's earlier agent
 * conversations, continue one in a new desktop tab, or delete one for good.
 * The phone names only the folder and the conversation; the desktop finds
 * it again in its own history.
 */
import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../transport/relayRpcMethods";
import { readStartResult } from "../tasks/readers";
import type { StartResult } from "../tasks/types";
import { readPastSessions, type PastSession } from "./model";

const plainFolder = (folder: string) => folder.length > 0 && folder.length <= 4096 && (folder.startsWith("/") || /^[A-Za-z]:\\/.test(folder)) && !folder.split(/[\\/]/).includes("..");
const plainRef = (provider: string, id: string) =>
  /^[a-z-]{1,32}$/.test(provider) && id.length > 0 && id.length <= 128 && [...id].every((char) => char.charCodeAt(0) >= 0x20);

export async function listPastSessions(client: RelayRpcClient, folder: string): Promise<readonly PastSession[]> {
  if (!plainFolder(folder)) throw new RelayRpcError("The relay request is invalid.");
  const sessions = readPastSessions(await client.request("history.list", { folder }, { timeoutMs: 30_000 }));
  if (!sessions) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return sessions;
}

export async function continuePastSession(client: RelayRpcClient, folder: string, session: PastSession): Promise<StartResult> {
  if (!plainFolder(folder) || !plainRef(session.provider, session.id)) throw new RelayRpcError("The relay request is invalid.");
  const result = readStartResult(await client.request("history.resume", { folder, provider: session.provider, id: session.id }, { timeoutMs: 30_000 }));
  if (!result) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return result;
}

export type DeleteResult =
  | { readonly deleted: true; readonly freed: number; readonly reusable: number }
  | { readonly deleted: false; readonly reason: string };

export async function deletePastSession(client: RelayRpcClient, folder: string, session: PastSession): Promise<DeleteResult> {
  if (!plainFolder(folder) || !plainRef(session.provider, session.id)) throw new RelayRpcError("The relay request is invalid.");
  // Deleting a big OpenCode conversation can take a while.
  const raw = await client.request("history.delete", { folder, provider: session.provider, id: session.id }, { timeoutMs: 150_000 });
  if (typeof raw !== "object" || raw === null) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  const value = raw as Record<string, unknown>;
  if (value.deleted === true) {
    const count = (n: unknown) => (typeof n === "number" && n >= 0 ? n : 0);
    return { deleted: true, freed: count(value.freed), reusable: count(value.reusable) };
  }
  return { deleted: false, reason: typeof value.reason === "string" ? value.reason.slice(0, 500) : "It wasn't deleted." };
}
