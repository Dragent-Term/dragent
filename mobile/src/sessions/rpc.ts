/**
 * Start a new agent session on the paired desktop (`sessions.start`): an
 * installed agent on a task in a project, in a new workspace (optionally
 * from a branch) or an existing one. The desktop does it through its own
 * new-workspace prompt, as if it had been started there.
 */
import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../transport/relayRpcMethods";
import { readStartResult } from "../tasks/readers";
import type { StartResult } from "../tasks/types";

export interface NewSession {
  /** The project's folder on the desktop. */
  readonly project: string;
  readonly agent: string;
  readonly prompt: string;
  readonly model?: string | null;
  /** An existing workspace folder; absent for a new workspace. */
  readonly workspace?: string | null;
  /** A new workspace's base branch; absent for the project's default. */
  readonly branch?: string | null;
}

export const MAX_SESSION_PROMPT = 8000;

/** The request as sent, or null when it can't be (checked before it leaves). */
export function sessionParams(input: NewSession): Record<string, string> | null {
  const prompt = input.prompt.trim();
  if (!prompt || prompt.length > MAX_SESSION_PROMPT || !input.project || !input.agent) return null;
  if (input.workspace && input.branch) return null;
  const params: Record<string, string> = { project: input.project, agent: input.agent, prompt };
  if (input.model) params.model = input.model;
  if (input.workspace) params.workspace = input.workspace;
  if (input.branch) params.branch = input.branch;
  return params;
}

export async function startSession(client: RelayRpcClient, input: NewSession): Promise<StartResult> {
  const params = sessionParams(input);
  if (!params) throw new RelayRpcError("The relay request is invalid.");
  const raw = await client.request("sessions.start", params, { timeoutMs: 30_000 });
  const result = readStartResult(raw);
  if (!result) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return result;
}
