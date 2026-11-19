import { ResourceRow as MachineRowShell, countLabel } from "../../components/ResourceRow";
import type { ReactNode } from "react";
import {
  AgentMark,
  BrowserMark,
  StatusBadge,
  TerminalMark,
  agentStateLabel,
  type AgentDotState,
} from "../../components";
import type { MachineSummary, RelayTab } from "../../transport/relayCatalog";
import {
  countNeeds,
  mostUrgent,
  needLabel,
  needsSummary,
  statusDot,
  type NeedCounts,
} from "../../machines/attention";

export { ResourceRow as MachineRowShell, countLabel } from "../../components/ResourceRow";

function lastSeenLabel(lastSeenAt: number): string {
  const age = Date.now() - lastSeenAt;
  if (!Number.isFinite(age) || age < 0) return "seen just now";
  if (age < 60_000) return "seen just now";
  const minutes = Math.floor(age / 60_000);
  if (minutes < 60) return `last seen ${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `last seen ${hours}h ago`;
  return `last seen ${Math.floor(hours / 24)}d ago`;
}

/** §6.3: a same-account desktop this phone has no pin for. No name, no catalog. */
export function PairDesktopRow({ onPair }: { onPair: () => void }) {
  return (
    <MachineRowShell
      icon="monitor"
      dot="idle"
      title="Pair this desktop to open it"
      detail="Same-account desktop · not paired on this phone"
      accessibilityLabel="Pair this desktop to open it, same-account desktop, not paired on this phone"
      onPress={onPair}
      fullBleed
    />
  );
}

/** One live machine from account discovery. */
export function MachineRow({
  machine,
  needs,
  onOpen,
}: {
  machine: MachineSummary;
  /** What its agents wait for, from its live catalog; omitted until known. */
  needs?: NeedCounts | undefined;
  onOpen: (deviceId: string) => void;
}) {
  const urgent = machine.online && needs ? mostUrgent(needs) : null;
  const summary = machine.online && needs ? needsSummary(needs) : null;
  // Online and offline both say so under the name, mirroring each other.
  const detail = machine.online
    ? `Online · ${machine.platform}`
    : `Offline · ${machine.platform} · ${lastSeenLabel(machine.last_seen_at)}`;
  return (
    <MachineRowShell
      icon="monitor"
      dot={machine.online ? "online" : "idle"}
      title={machine.name}
      detail={machine.version ? `${detail} · v${machine.version}` : detail}
      accessibilityLabel={`${machine.name}, ${machine.online ? "online" : "offline"}, ${machine.platform}${summary ? `, ${summary}` : ""}`}
      onPress={() => onOpen(machine.device_id)}
      fullBleed
      trailing={
        urgent && needs ? (
          <StatusBadge
            label={needLabel(urgent, needs)}
            tone={urgent === "review" ? "success" : "danger"}
            accessibilityLabel={summary ?? needLabel(urgent, needs)}
          />
        ) : undefined
      }
    />
  );
}

/** One live project with its explicit workspace/tab counts. */
export function LiveProjectRow({
  name,
  workspaceCount,
  tabCount,
  needs,
  onOpen,
}: {
  name: string;
  workspaceCount: number;
  tabCount: number;
  needs?: NeedCounts | undefined;
  onOpen: () => void;
}) {
  const summary = needs ? needsSummary(needs) : null;
  const detail =
    `${countLabel(workspaceCount, "workspace", "workspaces")} · ` +
    countLabel(tabCount, "open tab", "open tabs");
  return (
    <MachineRowShell
      icon="folder"
      dot={needs ? statusDot(needs) : null}
      title={name}
      detail={detail}
      accessibilityLabel={`${name}, ${summary ? `${summary}, ` : ""}${detail}`}
      onPress={onOpen}
    />
  );
}

/** One live workspace: its own name and branch plus explicit tab count. */
export function LiveWorkspaceRow({
  name,
  branch,
  tabCount,
  needs,
  onOpen,
}: {
  name: string;
  branch: string;
  tabCount: number;
  needs?: NeedCounts | undefined;
  onOpen: () => void;
}) {
  const summary = needs ? needsSummary(needs) : null;
  const detail = `${branch} · ${countLabel(tabCount, "open tab", "open tabs")}`;
  return (
    <MachineRowShell
      icon="branch"
      dot={needs ? statusDot(needs) : null}
      title={name}
      detail={detail}
      accessibilityLabel={`${name}, branch ${branch}, ${summary ? `${summary}, ` : ""}${detail}`}
      onPress={onOpen}
    />
  );
}

function liveAgentDot(tab: RelayTab): AgentDotState {
  return statusDot(countNeeds([tab])) ?? "idle";
}

function liveTabKindLabel(tab: RelayTab): string {
  if (tab.kind === "agent") return tab.agent_kind ? `${tab.agent_kind} tab` : "Agent tab";
  if (tab.kind === "terminal") return tab.agent_kind ? `Terminal · ${tab.agent_kind}` : "Terminal tab";
  if (tab.kind === "browser") return "Browser tab";
  return "Settings tab";
}

function liveTabMark(tab: RelayTab): ReactNode {
  const agent = (tab.agent_kind ?? "").trim().toLowerCase();
  if (tab.kind === "agent" || (tab.kind === "terminal" && agent !== "")) {
    if (agent !== "") {
      return <AgentMark agent={agent} size={22} />;
    }
  }
  if (tab.kind === "terminal" || tab.kind === "agent") return <TerminalMark size={22} />;
  if (tab.kind === "browser") return <BrowserMark size={20} />;
  return undefined;
}

/** One live tab: real kind, explicit agent identity/state, owning context. */
export function LiveTabRow({
  tab,
  context,
  onOpen,
}: {
  tab: RelayTab;
  context: string;
  onOpen: () => void;
}) {
  const dot = liveAgentDot(tab);
  const kindLabel = liveTabKindLabel(tab);
  const live = dot !== "idle" ? agentStateLabel(dot) : null;
  const detail = `${context} · ${kindLabel}${live ? ` · ${live}` : ""}`;
  const mark = liveTabMark(tab);
  return (
    <MachineRowShell
      icon="terminal"
      iconNode={mark}
      dot={dot}
      title={tab.title || kindLabel}
      detail={detail}
      accessibilityLabel={`${kindLabel}: ${tab.title || kindLabel}, ${detail}`}
      onPress={onOpen}
    />
  );
}

