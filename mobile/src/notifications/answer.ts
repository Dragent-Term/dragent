/** Answer an agent's permission prompt from its notification (`agent.answer`). */
import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../transport/relayRpcMethods";

export type AnswerChoice = "allow" | "deny";

export const ANSWER_CATEGORY = "dragent-answer";

/** A pane id is a UUID. */
export function isPaneId(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
}

export function isChoice(value: unknown): value is AnswerChoice {
  return value === "allow" || value === "deny";
}

export async function answerAgent(
  client: RelayRpcClient,
  pane: string,
  choice: AnswerChoice,
): Promise<{ answered: true } | { answered: false; reason: string }> {
  if (!isPaneId(pane) || !isChoice(choice)) throw new RelayRpcError("The relay request is invalid.");
  const raw = await client.request("agent.answer", { pane: pane.toLowerCase(), answer: choice }, { timeoutMs: 20_000 });
  if (typeof raw !== "object" || raw === null) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  const value = raw as Record<string, unknown>;
  if (value.answered === true) return { answered: true };
  return { answered: false, reason: typeof value.reason === "string" ? value.reason.slice(0, 300) : "It wasn't answered." };
}
