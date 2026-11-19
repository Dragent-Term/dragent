import { useCallback, useEffect, useState } from "react";
import { StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AgentMark, AppText, Button, ConfirmationPanel, LoadingState, RefreshButton, StatusBadge } from "../../components";
import { FieldInput } from "../../components/FieldInput";
import { ScrollScreen } from "../../layout";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import { machinePath, machineTabPath } from "../../machines/routes";
import { MachineHeader } from "./MachineHeader";
import { Feedback } from "../shared/Feedback";
import { limitTone, resetsIn } from "../../usage/types";
import {
  addAccount,
  chooseAccount,
  listAccounts,
  removeAccount,
  renameAccount,
  startSignIn,
  type AgentAccount,
  type AgentAccounts,
} from "../../accounts/rpc";

/**
 * The desktop's agent accounts: each Claude Code and Codex account, whether
 * it is signed in, its plan limits, and which one new sessions use. Add,
 * rename, remove, choose, and sign in (the agent's own login, run in a
 * desktop terminal this opens).
 */
export function MachineAccountsScreen({ deviceId }: { deviceId: string }) {
  const { theme } = useTheme();
  const machine = useMachine();
  const [agents, setAgents] = useState<readonly AgentAccounts[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!machine.rpc) return;
    setLoading(true);
    try {
      setAgents(await listAccounts(machine.rpc));
      setError(null);
    } catch {
      setError("The accounts couldn't be read. Check the desktop is online, then refresh.");
    } finally {
      setLoading(false);
    }
  }, [machine.rpc]);
  useEffect(() => {
    void load();
  }, [load]);

  /** Run a change; the desktop answers with every account as it is now. */
  const change = async (key: string, work: () => Promise<readonly AgentAccounts[]>, failed: string) => {
    setBusy(key);
    setError(null);
    try {
      setAgents(await work());
      return true;
    } catch {
      setError(failed);
      return false;
    } finally {
      setBusy(null);
    }
  };

  const signIn = async (account: AgentAccount) => {
    if (!machine.rpc) return;
    setBusy(`signin:${account.account}`);
    setError(null);
    try {
      const tab = await startSignIn(machine.rpc, account.account);
      router.push(machineTabPath(deviceId, tab));
    } catch {
      setError("The sign-in didn't start. Check the desktop is online, then try again.");
    } finally {
      setBusy(null);
    }
  };

  const back = machinePath(deviceId);
  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <MachineHeader
        title="Agent accounts"
        subtitle={machine.catalog.status === "live" ? (machine.catalog.catalog.machine?.name ?? machine.catalog.machine.name) : "Desktop"}
        online={machine.catalog.status === "live" ? machine.catalog.machine.online : null}
        onBack={() => (router.canGoBack() ? router.back() : router.navigate({ pathname: back.pathname, params: back.params }))}
        backLabel="Back"
        actions={<RefreshButton label="Check the accounts again" onPress={() => void load()} refreshing={loading} disabled={!machine.rpc} />}
      />
      {!agents && loading ? (
        <LoadingState label="Checking the accounts on your desktop…" />
      ) : (
        <ScrollScreen maxWidth={760} contentStyle={styles.content}>
          <AppText variant="caption" tone="textMuted">
            Each account keeps its own sign-in on your desktop. New sessions of an agent use the account marked In use.
          </AppText>
          <Feedback message={error} />
          {(agents ?? []).map((agent) => (
            <AgentCard
              key={agent.provider}
              agent={agent}
              busy={busy}
              onUse={(account) =>
                void change(`use:${account.account}`, () => chooseAccount(machine.rpc!, agent.provider, account.account), "That account wasn't chosen. Try again.")
              }
              onRename={(account, label) =>
                change(`rename:${account.account}`, () => renameAccount(machine.rpc!, account.account, label), "It wasn't renamed. Names are 1–64 characters, one per agent.")
              }
              onRemove={(account) =>
                void change(`remove:${account.account}`, () => removeAccount(machine.rpc!, account.account), "It wasn't removed. Try again.")
              }
              onAdd={(label) =>
                change(`add:${agent.provider}`, () => addAccount(machine.rpc!, agent.provider, label), "It wasn't added. Names are 1–64 characters, one per agent.")
              }
              onSignIn={(account) => void signIn(account)}
            />
          ))}
          {agents && agents.length === 0 ? <AppText tone="textMuted">No agent on this desktop keeps separate accounts.</AppText> : null}
        </ScrollScreen>
      )}
    </View>
  );
}

function AgentCard({
  agent,
  busy,
  onUse,
  onRename,
  onRemove,
  onAdd,
  onSignIn,
}: {
  agent: AgentAccounts;
  busy: string | null;
  onUse: (account: AgentAccount) => void;
  onRename: (account: AgentAccount, label: string) => Promise<boolean>;
  onRemove: (account: AgentAccount) => void;
  onAdd: (label: string) => Promise<boolean>;
  onSignIn: (account: AgentAccount) => void;
}) {
  const { theme } = useTheme();
  const [adding, setAdding] = useState("");
  const several = agent.accounts.length > 1;
  return (
    <View style={[styles.card, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
      <View style={styles.row}>
        <AgentMark agent={agent.provider} size={24} />
        <AppText style={styles.agentName}>{agent.name}</AppText>
      </View>
      {agent.accounts.map((account) => (
        <AccountRow
          key={account.account}
          account={account}
          several={several}
          busy={busy}
          onUse={() => onUse(account)}
          onRename={(label) => onRename(account, label)}
          onRemove={() => onRemove(account)}
          onSignIn={() => onSignIn(account)}
        />
      ))}
      {agent.supported ? (
        <View style={styles.add}>
          <FieldInput
            containerStyle={styles.grow}
            value={adding}
            onChangeText={setAdding}
            placeholder="Add an account, e.g. Work"
            accessibilityLabel={`New ${agent.name} account name`}
            maxLength={64}
            autoCapitalize="words"
          />
          <Button
            label="Add"
            size="sm"
            loading={busy === `add:${agent.provider}`}
            disabled={!adding.trim() || busy !== null}
            onPress={() => void onAdd(adding).then((ok) => { if (ok) setAdding(""); })}
          />
        </View>
      ) : (
        <AppText variant="caption" tone="textMuted">{agent.reason || "Separate accounts aren't available for this agent here."}</AppText>
      )}
    </View>
  );
}

function AccountRow({
  account,
  several,
  busy,
  onUse,
  onRename,
  onRemove,
  onSignIn,
}: {
  account: AgentAccount;
  several: boolean;
  busy: string | null;
  onUse: () => void;
  onRename: (label: string) => Promise<boolean>;
  onRemove: () => void;
  onSignIn: () => void;
}) {
  const { theme } = useTheme();
  const [renaming, setRenaming] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const signedIn = account.status === "Signed in" || account.status.startsWith("System login");
  const statusColor = signedIn ? theme.colors.success : account.status === "Sign-in required" ? theme.colors.warning : theme.colors.textMuted;
  const now = Date.now();
  return (
    <View style={[styles.account, { borderColor: account.inUse && several ? theme.colors.accent : theme.colors.border }]}>
      {renaming !== null ? (
        <View style={styles.add}>
          <FieldInput containerStyle={styles.grow} value={renaming} onChangeText={setRenaming} accessibilityLabel="Account name" maxLength={64} autoFocus />
          <Button label="Save" size="sm" loading={busy === `rename:${account.account}`} disabled={!renaming.trim()} onPress={() => void onRename(renaming).then((ok) => { if (ok) setRenaming(null); })} />
          <Button label="Cancel" size="sm" variant="ghost" onPress={() => setRenaming(null)} />
        </View>
      ) : (
        <View style={styles.row}>
          <View style={styles.grow}>
            <AppText style={styles.label} numberOfLines={1}>{account.label}</AppText>
            <View style={styles.row}>
              <View style={[styles.dot, { backgroundColor: statusColor }]} />
              <AppText variant="caption" tone="textMuted" numberOfLines={2} style={styles.grow}>
                {[account.status || "Not checked", account.plan ? account.plan.charAt(0).toUpperCase() + account.plan.slice(1) : null].filter(Boolean).join(" · ")}
              </AppText>
            </View>
          </View>
          {account.inUse && several ? <StatusBadge label="In use" tone="accent" /> : null}
        </View>
      )}
      {account.windows.map((window) => (
        <View key={window.label} style={styles.meterRow}>
          <AppText variant="caption" tone="textMuted" style={styles.meterLabel}>{window.label}</AppText>
          <View style={[styles.meter, { backgroundColor: theme.colors.surfaceRaised }]}>
            <View style={[styles.meterFill, { width: `${Math.max(2, window.usedPercent)}%`, backgroundColor: theme.colors[limitTone(window.usedPercent)] }]} />
          </View>
          <AppText variant="caption" tone="textMuted">
            {`${Math.round(window.usedPercent)}%${resetsIn(window.resetsAt, now) ? ` · ${resetsIn(window.resetsAt, now)}` : ""}`}
          </AppText>
        </View>
      ))}
      {removing ? (
        <ConfirmationPanel
          title={`Remove ${account.label}?`}
          message="It comes off this desktop's list. Its sign-in and history stay with the agent; new sessions that used it go back to the system login."
          actionLabel="Remove"
          onCancel={() => setRemoving(false)}
          onConfirm={() => {
            setRemoving(false);
            onRemove();
          }}
        />
      ) : (
        <View style={styles.actions}>
          {!account.inUse ? (
            <Button label="Use for new sessions" size="sm" variant="ghost" loading={busy === `use:${account.account}`} disabled={busy !== null} onPress={onUse} />
          ) : null}
          {!account.system ? (
            <Button
              label={signedIn ? "Sign in again" : "Sign in"}
              size="sm"
              variant={signedIn ? "ghost" : "primary"}
              loading={busy === `signin:${account.account}`}
              disabled={busy !== null}
              onPress={onSignIn}
            />
          ) : null}
          {!account.system ? <Button label="Rename" size="sm" variant="ghost" disabled={busy !== null} onPress={() => setRenaming(account.label)} /> : null}
          {!account.system ? <Button label="Remove" size="sm" variant="ghost" disabled={busy !== null} onPress={() => setRemoving(true)} /> : null}
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1 },
  content: { gap: spacing.md },
  card: { borderWidth: 1, borderRadius: radii.xl, borderCurve: "continuous", padding: spacing.md, gap: spacing.sm },
  row: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  agentName: { fontWeight: "600", fontSize: 16 },
  account: { borderWidth: 1, borderRadius: radii.lg, borderCurve: "continuous", padding: spacing.sm, gap: spacing.xs },
  label: { fontWeight: "600" },
  grow: { flex: 1, minWidth: 0 },
  dot: { width: 7, height: 7, borderRadius: 4 },
  meterRow: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  meterLabel: { width: 64 },
  meter: { flex: 1, height: 6, borderRadius: 3, overflow: "hidden" },
  meterFill: { height: 6, borderRadius: 3 },
  actions: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: spacing.xs },
  add: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
});
