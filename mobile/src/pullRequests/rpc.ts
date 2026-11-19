import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../transport/relayRpcMethods";
import { readPullRequest, type PullRequestReading } from "./model";

/** The workspace branch's pull request and checks (`git.pr_status {path}`). */
export async function readPullRequestStatus(client: RelayRpcClient, root: string): Promise<PullRequestReading> {
  if (!root.startsWith("/") || root.split("/").includes("..")) throw new RelayRpcError("The relay request is invalid.");
  const reading = readPullRequest(await client.request("git.pr_status", { path: root }, { timeoutMs: 30_000 }));
  if (!reading) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return reading;
}
