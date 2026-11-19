/**
 * The desktop's agent accounts (`accounts.*`), served inside the encrypted
 * channel. Credentials never leave the desktop: signing in runs the agent's
 * own login in a desktop terminal the phone opens.
 */
import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE, createTerminal, sendTabText } from "../transport/relayRpcMethods";

export interface AgentAccount {
  readonly account: string;
  readonly label: string;
  readonly system: boolean;
  readonly inUse: boolean;
  /** "Signed in", "Sign-in required", or why it couldn't be checked. */
  readonly status: string;
  readonly plan: string | null;
  readonly windows: readonly { readonly label: string; readonly usedPercent: number; readonly resetsAt: number | null }[];
}

export interface AgentAccounts {
  readonly provider: string;
  readonly name: string;
  /** Separate accounts work for this agent on this desktop. */
  readonly supported: boolean;
  readonly reason: string;
  readonly accounts: readonly AgentAccount[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function str(value: unknown, max: number): string | null {
  return typeof value === "string" && value.length <= max ? value : null;
}

function readAccount(value: unknown): AgentAccount | null {
  if (!isRecord(value)) return null;
  const account = str(value.account, 64);
  const label = str(value.label, 200);
  if (!account || label === null || !Array.isArray(value.windows)) return null;
  const windows = value.windows.slice(0, 10).flatMap((entry) => {
    if (!isRecord(entry) || typeof entry.label !== "string" || typeof entry.used_percent !== "number") return [];
    return [Object.freeze({
      label: entry.label.slice(0, 64),
      usedPercent: Math.max(0, Math.min(100, entry.used_percent)),
      resetsAt: typeof entry.resets_at === "number" ? entry.resets_at : null,
    })];
  });
  return Object.freeze({
    account,
    label,
    system: value.system === true,
    inUse: value.in_use === true,
    status: (str(value.status, 300) ?? "").trim(),
    plan: str(value.plan, 64),
    windows: Object.freeze(windows),
  });
}

export function readAgentAccounts(raw: unknown): readonly AgentAccounts[] | null {
  if (!isRecord(raw) || !Array.isArray(raw.agents) || raw.agents.length > 10) return null;
  const agents: AgentAccounts[] = [];
  for (const entry of raw.agents) {
    if (!isRecord(entry) || !Array.isArray(entry.accounts)) return null;
    const provider = str(entry.provider, 32);
    const name = str(entry.name, 64);
    if (!provider || !name) return null;
    const accounts: AgentAccount[] = [];
    for (const row of entry.accounts.slice(0, 50)) {
      const account = readAccount(row);
      if (!account) return null;
      accounts.push(account);
    }
    agents.push(Object.freeze({
      provider,
      name,
      supported: entry.supported === true,
      reason: str(entry.reason, 600) ?? "",
      accounts: Object.freeze(accounts),
    }));
  }
  return Object.freeze(agents);
}

async function call(client: RelayRpcClient, method: string, params: Record<string, unknown>): Promise<readonly AgentAccounts[]> {
  let raw: unknown;
  try {
    // Checking each account's sign-in runs the agent's CLI on the desktop.
    raw = await client.request(method, params, { timeoutMs: 45_000 });
  } catch (error) {
    if (error instanceof RelayRpcError) throw error;
    throw new RelayRpcError("The request failed.");
  }
  const agents = readAgentAccounts(raw);
  if (!agents) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return agents;
}

const ACCOUNT = /^(system|[0-9a-f-]{36})$/;

export const listAccounts = (client: RelayRpcClient) => call(client, "accounts.list", {});
export const addAccount = (client: RelayRpcClient, provider: string, label: string) =>
  call(client, "accounts.add", { provider, label: label.trim().slice(0, 64) });
export const renameAccount = (client: RelayRpcClient, account: string, label: string) =>
  call(client, "accounts.rename", { account, label: label.trim().slice(0, 64) });
export const removeAccount = (client: RelayRpcClient, account: string) => call(client, "accounts.remove", { account });
export function chooseAccount(client: RelayRpcClient, provider: string, account: string) {
  if (!ACCOUNT.test(account)) return Promise.reject(new RelayRpcError("The request is invalid."));
  return call(client, "accounts.use", { provider, account });
}

/**
 * Sign an account in: open a desktop terminal and run the agent's own login
 * there for that account. Returns the terminal's tab, which the phone opens
 * so you can follow the login (its link or code).
 */
export async function startSignIn(client: RelayRpcClient, account: string): Promise<number> {
  if (!/^[0-9a-f-]{36}$/.test(account)) throw new RelayRpcError("The request is invalid.");
  const { id } = await createTerminal(client, { timeoutMs: 20_000 });
  // The new tab reaches the desktop's routes a moment after it opens.
  for (let attempt = 0; ; attempt += 1) {
    try {
      await sendTabText(client, id, `dragent agents accounts login ${account}`);
      return id;
    } catch (error) {
      if (attempt >= 8) throw error;
      await new Promise((resolve) => setTimeout(resolve, 600));
    }
  }
}
