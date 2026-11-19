/** Typed `usage.*` calls, served by the paired desktop inside the encrypted channel. */
import { useCallback } from "react";
import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../transport/relayRpcMethods";
import { useMachineResource } from "../coordination/useLiveCoordination";
import { readAccountsReply, readUsageSummary } from "./readers";
import type { UsageAccount, UsageSummary } from "./types";

const ACCOUNT_ID = /^[A-Za-z0-9_-]{1,64}$/;

async function call<T>(client: RelayRpcClient, method: string, params: Record<string, unknown>, read: (raw: unknown) => T | null): Promise<T> {
  let raw: unknown;
  try {
    // Reading every agent's records can take a while on a busy desktop.
    raw = await client.request(method, params, { timeoutMs: 45_000 });
  } catch (error) {
    if (error instanceof RelayRpcError) throw error;
    throw new RelayRpcError("The request failed.");
  }
  const parsed = read(raw);
  if (parsed === null) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return parsed;
}

export function getUsage(client: RelayRpcClient, days: number, refresh = false): Promise<UsageSummary> {
  if (!Number.isInteger(days) || days < 1 || days > 365) return Promise.reject(new RelayRpcError("The request is invalid."));
  return call(client, "usage.summary", { days, ...(refresh ? { refresh: true } : {}) }, readUsageSummary);
}

/** Make `account` the one new sessions of `provider` use on the desktop. */
export function chooseUsageAccount(client: RelayRpcClient, provider: string, account: string): Promise<readonly UsageAccount[]> {
  if ((provider !== "claude" && provider !== "codex") || !ACCOUNT_ID.test(account)) {
    return Promise.reject(new RelayRpcError("The request is invalid."));
  }
  return call(client, "usage.use_account", { provider, account }, readAccountsReply);
}

/** Usage changes slowly; a minute between reads is plenty. */
export const USAGE_POLL_MS = 60_000;

export function useLiveUsage(days: number) {
  const fetch = useCallback((client: RelayRpcClient) => getUsage(client, days), [days]);
  return useMachineResource<UsageSummary>({ scope: `usage\u0000${days}`, fetch, pollMs: USAGE_POLL_MS });
}
