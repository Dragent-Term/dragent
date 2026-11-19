import { router } from "expo-router";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Pressable, StyleSheet, View } from "react-native";
import { AgentMark, AgentStateDot, AppText, Button, EmptyState, Icon, LoadingState, RefreshButton, Section } from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { machineTabPath } from "../../machines/routes";
import { workspaceKey } from "../../machines/machineCatalog";
import { homeAttention } from "../../machines/workspaceDirectory";
import { useDevice } from "../../pairing/DeviceContext";
import { useMachines } from "../../machines/useMachines";
import { machinePath, machineWorkspacesShowing } from "../../machines/routes";
import { Feedback } from "../shared/Feedback";
import { MachineRow, PairDesktopRow } from "./MachineRows";
import { displayMachineName } from "../../machines/machineCatalog";
import { phoneSecurity } from "../../transport/e2ee/native";
import { MachineProvider, useMachine } from "../../machines/MachineContext";
import { countNeeds, needDot, tabNeed } from "../../machines/attention";
import type { MachineSummary, RelayCatalog, RelayTab } from "../../transport/relayCatalog";

/**
 * Paired desktops for Home: real desktop entries from `machines.list`
 * behind the phone's pairing. Loading, offline/error, empty and unpaired
 * are distinct, and failures never substitute sample data.
 */
export function MachinesSection() {
  const device = useDevice();
  const machines = useMachines();
  const openMachine = useCallback((deviceId: string) => {
    router.push(machinePath(deviceId));
  }, []);
  // §2.2: post-cut-over the relay sends no names; paired names come from
  // the pin store. Null on builds without E2EE (web, missing module).
  const security = useMemo(() => {
    try {
      return phoneSecurity();
    } catch {
      return null;
    }
  }, []);

  // A full refresh: the desktop list again, and each online desktop's
  // projects, workspaces and tabs. The icon turns until all have answered.
  const [refreshTick, setRefreshTick] = useState(0);
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const [listPending, setListPending] = useState(false);
  const listed = machines.machines;
  useEffect(() => setListPending(false), [listed]);
  useEffect(() => {
    if (!listPending && pending.size === 0) return;
    // Never turn forever on a desktop that doesn't answer.
    const cap = setTimeout(() => { setListPending(false); setPending(new Set()); }, 12_000);
    return () => clearTimeout(cap);
  }, [listPending, pending.size]);
  const refreshAll = useCallback(() => {
    setPending(new Set((listed ?? []).filter((machine) => machine.online).map((machine) => machine.device_id)));
    setListPending(true);
    setRefreshTick((tick) => tick + 1);
    machines.reconnect();
  }, [listed, machines]);
  const refreshed = useCallback((deviceId: string) => {
    setPending((current) => {
      if (!current.has(deviceId)) return current;
      const next = new Set(current);
      next.delete(deviceId);
      return next;
    });
  }, []);
  const refreshing = machines.refreshing || listPending || pending.size > 0;

  if (device.status !== "paired") {
    return (
      <Section title="Desktops" description="Your desktops' live projects and tabs.">
        <View style={styles.signedOut}>
          <AppText variant="caption" tone="textMuted">
            {device.ready
              ? "Pair this phone with your desktop to see its live projects and tabs."
              : "Loading pairing…"}
          </AppText>
          {device.ready ? (
            <Button label="Pair your phone" size="sm" variant="ghost" onPress={() => router.push("/pair-scan")} />
          ) : null}
        </View>
      </Section>
    );
  }

  if (machines.status === "loading") {
    return (
      <Section title="Desktops" description="Your desktops' live projects and tabs.">
        <LoadingState label="Looking for your desktops…" />
      </Section>
    );
  }

  if (machines.status === "unavailable") {
    return (
      <Section title="Desktops" description="Your desktops' live projects and tabs.">
        <Feedback message={machines.reason} />
        <Button label={machines.sessionEnded ? "Start new session" : "Retry"} size="sm" variant="ghost" onPress={machines.reconnect} />
      </Section>
    );
  }

  if (machines.machines.length === 0) {
    return (
      <Section title="Desktops" description="Your desktops' live projects and tabs.">
        <EmptyState
          title="No desktops sharing yet"
          message="When a paired desktop shares its live catalog, it appears here."
          actionLabel="Retry"
          onAction={machines.reconnect}
        />
      </Section>
    );
  }

  return (
    <Section
      title="Desktops"
      description="Your desktops' live projects and tabs."
      actions={
        <RefreshButton label="Refresh your desktops" onPress={refreshAll} refreshing={refreshing} />
      }
    >
      <View>
        {machines.machines.map((machine) => {
          // §6.3: entries still nameless after the pin-store lookup are
          // same-account desktops with no pin. No name is shown and no
          // catalog is opened for them.
          const name = displayMachineName(machine, security?.desktopName(machine.device_id) ?? null);
          if (name.trim() === "") {
            return (
              <PairDesktopRow
                key={machine.device_id}
                onPair={() => router.push("/pair-scan")}
              />
            );
          }
          const named = name === machine.name ? machine : { ...machine, name };
          return named.online ? (
            // Its live catalog says what its agents wait for.
            <MachineProvider key={machine.device_id} deviceId={machine.device_id}>
              <LiveMachineRow
                machine={named}
                onOpen={openMachine}
                refreshTick={refreshTick}
                refreshing={pending.has(machine.device_id)}
                onRefreshed={refreshed}
              />
            </MachineProvider>
          ) : (
            <MachineRow key={machine.device_id} machine={named} onOpen={openMachine} />
          );
        })}
      </View>
    </Section>
  );
}

function LiveMachineRow({
  machine,
  onOpen,
  refreshTick,
  refreshing,
  onRefreshed,
}: {
  machine: MachineSummary;
  onOpen: (deviceId: string) => void;
  /** Bumped by home's refresh: load this desktop's catalog again. */
  refreshTick: number;
  /** Home waits on this desktop's new catalog. */
  refreshing: boolean;
  onRefreshed: (deviceId: string) => void;
}) {
  const { catalog, reconnect } = useMachine();
  const seenTick = useRef(refreshTick);
  const sawLoading = useRef(false);
  useEffect(() => {
    if (refreshTick === seenTick.current) return;
    seenTick.current = refreshTick;
    sawLoading.current = false;
    reconnect();
  }, [refreshTick, reconnect]);
  const deviceId = machine.device_id;
  useEffect(() => {
    if (!refreshing) return;
    if (catalog.status === "loading") {
      sawLoading.current = true;
      return;
    }
    if (catalog.status !== "live") {
      onRefreshed(deviceId);
      return;
    }
    if (sawLoading.current) {
      onRefreshed(deviceId);
      return;
    }
    // The catalog came back without a visible reload: give it a moment.
    const settle = setTimeout(() => onRefreshed(deviceId), 1200);
    return () => clearTimeout(settle);
  }, [refreshing, catalog.status, deviceId, onRefreshed]);
  const needs = catalog.status === "live" ? countNeeds(catalog.catalog.tabs) : undefined;
  // Only what's blocked on the person gets a row of its own; finished turns
  // are one count, so many agents finishing don't flood home.
  const attention = catalog.status === "live" ? homeAttention(catalog.catalog) : null;
  return (
    <View>
      <MachineRow machine={machine} needs={needs} onOpen={onOpen} />
      {attention?.urgent.map((tab) => (
        <NeedsYouRow
          key={tab.id}
          tab={tab}
          context={tabContext(catalog.status === "live" ? catalog.catalog : null, tab)}
          onOpen={() => router.push(machineTabPath(machine.device_id, tab.id))}
        />
      ))}
      {attention && attention.moreUrgent > 0 ? (
        <SummaryRow
          dot="waiting"
          label={`${attention.moreUrgent} more ${attention.moreUrgent === 1 ? "needs" : "need"} you`}
          onOpen={() => router.push(machineWorkspacesShowing(machine.device_id, "needs_you"))}
        />
      ) : null}
      {attention && attention.reviews > 0 ? (
        <SummaryRow
          dot="done"
          label={`${attention.reviews} ready to review`}
          onOpen={() => router.push(machineWorkspacesShowing(machine.device_id, "review"))}
        />
      ) : null}
    </View>
  );
}

/** One line standing for several tabs: opens the desktop's list filtered to them. */
function SummaryRow({ dot, label, onOpen }: { dot: "waiting" | "done"; label: string; onOpen: () => void }) {
  const { theme } = useTheme();
  return (
    <Pressable
      onPress={onOpen}
      accessibilityRole="button"
      accessibilityLabel={`${label}. Shows them on this desktop.`}
      style={({ pressed }) => [
        styles.needRow,
        styles.summaryRow,
        { borderColor: theme.colors.border, backgroundColor: pressed ? theme.colors.surfaceRaised : theme.colors.background },
      ]}
    >
      <AgentStateDot state={dot} />
      <AppText variant="caption" tone={dot === "waiting" ? "danger" : "textMuted"} style={styles.needText} numberOfLines={1}>
        {label}
      </AppText>
      <Icon name="chevron-right" size={16} tone="textSubtle" />
    </Pressable>
  );
}

function tabContext(catalog: RelayCatalog | null, tab: RelayTab): string {
  const workspace = catalog?.workspaces.find((entry) => workspaceKey(entry) === tab.workspace_id);
  return workspace ? workspace.name : "";
}

/** One tab waiting for the person, indented under its desktop. */
function NeedsYouRow({ tab, context, onOpen }: { tab: RelayTab; context: string; onOpen: () => void }) {
  const { theme } = useTheme();
  const need = tabNeed(tab);
  const what = need === "needs_input" ? "Needs your input" : need === "failed" ? "Failed" : "Ready to review";
  return (
    <Pressable
      onPress={onOpen}
      accessibilityRole="button"
      accessibilityLabel={`${tab.title || "Tab"}${context ? ` in ${context}` : ""}: ${what}`}
      style={({ pressed }) => [
        styles.needRow,
        { borderColor: theme.colors.border, backgroundColor: pressed ? theme.colors.surfaceRaised : theme.colors.background },
      ]}
    >
      <AgentStateDot state={need ? needDot(need) : "idle"} />
      {tab.agent_kind ? <AgentMark agent={tab.agent_kind} size={16} /> : null}
      <View style={styles.needText}>
        <AppText variant="body" numberOfLines={1}>{tab.title || "Tab"}</AppText>
        <AppText variant="caption" tone={need === "review" ? "textMuted" : "danger"} numberOfLines={1}>
          {what}{context ? ` · ${context}` : ""}
        </AppText>
      </View>
      <Icon name="chevron-right" size={16} tone="textSubtle" />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  needRow: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    minHeight: 52,
    paddingVertical: spacing.xs,
    paddingLeft: spacing.xl + spacing.md,
    paddingRight: spacing.sm,
    borderBottomWidth: 1,
  },
  needText: { flex: 1, minWidth: 0 },
  summaryRow: { minHeight: 40 },
  signedOut: { gap: 8, alignItems: "flex-start" },
});
