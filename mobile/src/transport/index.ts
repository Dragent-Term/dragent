export type { RealtimeClock } from "./clock";
export {
  RelayRpcClient, RelayRpcError,
  RELAY_RPC_CONNECTION_LOST_MESSAGE, RELAY_RPC_DEFAULT_TIMEOUT_MS,
  RELAY_RPC_FAILED_MESSAGE, RELAY_RPC_SEND_FAILED_MESSAGE, RELAY_RPC_TIMEOUT_MESSAGE,
} from "./relayRpcClient";
export type { RelayRpcClientOptions, RelayStreamHandler, RelaySubscription } from "./relayRpcClient";
export {
  fetchRelayCatalog, isRelayAgentState, isRelayTabKind, parseMachinesListResult, parseMachineSummary,
  parseRelayCatalog, parseRelayCatalogSnapshotPayload, resolveRelayCatalogURL,
  RelayCatalogError, RELAY_CATALOG_LIMITS,
} from "./relayCatalog";
export {
  createTerminal, createWorktree, listFiles, listGithubWorkItems, listGitlabWorkItems, listMachines,
  listTabs, listWorktrees, openFile, parseRelayRpcAttentionEvent, parseRelayRpcEventsSnapshot,
  parseRelayRpcFileContentResult, parseRelayRpcFilesResult, parseRelayRpcTerminalRef,
  parseRelayRpcTabsResult, parseRelayRpcWorkItemsResult, parseRelayRpcWorktreeEntry,
  parseRelayRpcWorktreeListResult, searchFiles, subscribeMachineCatalog, subscribeRuntimeEvents,
  isRelayRpcMethod, isRelayRpcTabKind, readRelayCatalog,
} from "./relayRpcMethods";
export type {
  MachineCatalogListener, RelayRpcAttentionEvent, RelayRpcAttentionKind, RelayRpcCreateWorktreeInput,
  RelayRpcFileContent, RelayRpcFileEntry, RelayRpcFileKind, RelayRpcListWorkItemsInput, RelayRpcTab,
  RelayRpcTabKind, RelayRpcTerminalRef, RelayRpcWorkItem, RelayRpcWorktree, RuntimeEventsListener,
} from "./relayRpcMethods";
export type {
  MachineSummary, RelayAgentState, RelayCatalog, RelayCatalogFetch, RelayCatalogOptions,
  RelayCatalogSnapshot, RelayMachine, RelayProject, RelayTab, RelayTabKind, RelayWorkspace,
} from "./relayCatalog";
export {
  initialMachineCatalogState, initialRelayCatalogState, loadingMachineCatalog, parseRelayCatalogSnapshot,
  unavailableMachineCatalog, useRelayCatalog,
} from "./useRelayCatalog";
export type { MachineCatalogState, RelayCatalogSource, RelayCatalogState } from "./useRelayCatalog";
export type { RealtimeSocket, SocketFactory, SocketOptions } from "./socket";
export type {
  Participant, Role, TerminalDelta, TerminalSurfaceState, WireAgentBlock, WireColor, WireLine, WireSpan,
} from "./protocol";

export { RelaySocketClient, resolveRelaySocketURL, RELAY_SOCKET_TIMING } from "./RelaySocketClient";
export type { RelaySocketOptions, RelaySocketSnapshot } from "./RelaySocketClient";
export { selectOwnedState, useRelaySocket } from "./useRelaySocket";
export type { OwnedResult, RelaySocketSource } from "./useRelaySocket";

// End-to-end encryption (docs/design/e2ee.md; task E5a).
export { socketE2ee, phoneSecurity, unpairDesktop } from "./e2ee/native";
export { PhoneSecurity, STORE_LOST_MESSAGE } from "./e2ee/security";
export type { CatalogCheck, PinStatus, UnpairOptions, UnpairResult } from "./e2ee/security";
export {
  E2eeDesktopLink, CHANNEL_LOST_CODE, INPUT_LOST_CODE, REPLY_FAILED_CODE, SEED_FAILED_CODE, SEED_FAILED_MESSAGE,
  inputLostMessage, linkProblemMessage,
} from "./e2ee/desktopLink";
export type { DesktopLinkProblem, DesktopLinkState, DesktopLinkStatus } from "./e2ee/desktopLink";
export { CATALOG_REJECTED_MESSAGE, openSealedCatalogPayload } from "./e2ee/sealedCatalog";
export { PAIR_AGAIN_MESSAGE, UPDATE_APP_MESSAGE } from "./RelaySocketClient";
export type { RelaySocketE2ee } from "./RelaySocketClient";
export { decodeCtrl, encodeCtrl } from "./e2ee/ctrl";
export type { Ctrl } from "./e2ee/ctrl";
