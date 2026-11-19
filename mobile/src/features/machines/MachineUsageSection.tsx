import { useEffect, useMemo, useState } from "react";
import { RefreshControl, ScrollView, StyleSheet, View } from "react-native";
import { AgentMark, AppText, Button, Control, LoadingState, RefreshButton, SegmentedControl, StatusBadge } from "../../components";
import { getAgentLabel, normalizeAgentKind } from "../../components/agentArtwork";
import { useResponsiveLayout } from "../../layout/useResponsiveLayout";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { Feedback } from "../shared/Feedback";
import { useMachine } from "../../machines/MachineContext";
import { router } from "expo-router";
import { machineAccountsPath } from "../../machines/routes";
import { chooseUsageAccount, getUsage, useLiveUsage } from "../../usage/rpc";
import {
  USAGE_RANGES,
  formatMeasure,
  limitTone,
  measure,
  resetsIn,
  shortCount,
  dollars,
  type UsageAccount,
  type UsageMetric,
  type UsageRange,
  type UsageSummary,
  type UsageTotals,
} from "../../usage/types";

const METRICS = [
  { id: "tokens", label: "Tokens", hint: "Show tokens" },
  { id: "cost", label: "Cost", hint: "Show estimated API cost" },
] as const;

/** How many rows a breakdown shows before "Show all". */
const FOLDED = 5;

const NAMES: Record<string, string> = { mimo: "MiMo Code", copilot: "GitHub Copilot", qwen: "Qwen Code", codex: "Codex", claude: "Claude Code" };

function agentName(agent: string): string {
  if (NAMES[agent]) return NAMES[agent];
  const kind = normalizeAgentKind(agent);
  return kind ? getAgentLabel(kind) : agent.charAt(0).toUpperCase() + agent.slice(1);
}

/**
 * The desktop's Usage page on the phone: what its coding agents used over a
 * range (tokens or estimated cost), by day, agent, model and project, and
 * each Claude Code and Codex account's plan limits, with the account new
 * sessions use. Read from the agents' own records on the desktop.
 */
export function MachineUsageSection() {
  const { theme } = useTheme();
  const layout = useResponsiveLayout();
  const machine = useMachine();
  const [range, setRange] = useState<UsageRange>("7");
  const [metric, setMetric] = useState<UsageMetric>("tokens");
  const days = Number(range);
  const live = useLiveUsage(days);
  // A refresh or an account change answers with fresh data before the next poll.
  const [override, setOverride] = useState<UsageSummary | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [switching, setSwitching] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const liveResult = live.state.status === "live" ? live.state.result : null;
  useEffect(() => setOverride(null), [liveResult, days]);
  const summary = override?.days === days ? override : liveResult;

  const colors = useMemo(() => {
    const palette = [theme.colors.accent, "#a78bfa", theme.colors.success, "#fb923c", theme.colors.warning, "#60a5fa", theme.colors.danger];
    const map = new Map<string, string>();
    (summary?.agents ?? []).forEach((entry, index) => map.set(entry.agent, palette[index % palette.length]!));
    return map;
  }, [summary, theme]);
  const colorOf = (agent: string) => colors.get(agent) ?? theme.colors.textMuted;

  const refresh = async () => {
    if (!machine.rpc) return;
    setRefreshing(true);
    setError(null);
    try {
      setOverride(await getUsage(machine.rpc, days, true));
    } catch {
      setError("Usage didn't refresh. Check the desktop is online, then try again.");
    } finally {
      setRefreshing(false);
    }
  };

  const useAccount = async (account: UsageAccount) => {
    if (!machine.rpc || !summary) return;
    setSwitching(`${account.provider}\u0000${account.account}`);
    setError(null);
    try {
      const accounts = await chooseUsageAccount(machine.rpc, account.provider, account.account);
      setOverride({ ...summary, accounts });
    } catch {
      setError(`New ${agentName(account.provider)} sessions still use the same account. Try again.`);
    } finally {
      setSwitching(null);
    }
  };

  const gutter = { paddingHorizontal: layout.gutter };
  const controls = (
    <View style={[styles.controls, gutter]}>
      <View style={styles.row}>
        <View style={styles.grow}>
          <SegmentedControl label="Range" options={USAGE_RANGES} value={range} size="sm" onChange={setRange} />
        </View>
        <RefreshButton label="Refresh usage" onPress={() => void refresh()} refreshing={refreshing} disabled={!machine.rpc} />
      </View>
      <SegmentedControl label="Measure" options={METRICS} value={metric} size="sm" onChange={setMetric} />
    </View>
  );

  if (!summary) {
    if (live.state.status === "unavailable") {
      const ended = live.state.action === "reconnect";
      return (
        <View style={styles.frame}>
          {controls}
          <View style={[styles.padded, gutter]}>
            <Feedback message={live.state.reason} />
            <Button label={ended ? "Start new session" : "Retry"} size="sm" variant="ghost" onPress={ended ? live.reconnect : live.retry} />
          </View>
        </View>
      );
    }
    return (
      <View style={styles.frame}>
        {controls}
        <LoadingState label="Reading the agents' usage on your desktop…" />
      </View>
    );
  }

  const total = measure(summary.total, metric);
  const claude = summary.accounts.filter((account) => account.provider === "claude");
  const codex = summary.accounts.filter((account) => account.provider === "codex");

  return (
    <View style={styles.frame}>
      {controls}
      <ScrollView
        style={styles.frame}
        contentContainerStyle={[styles.content, gutter]}
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} tintColor={theme.colors.textMuted} />}
      >
        <Feedback message={error} />

        {/* Total: the headline number and what it's made of. */}
        <View style={[styles.card, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
          <AppText variant="caption" tone="textMuted">
            {range === "1" ? "Today" : `Last ${days} days`}
            {metric === "cost" ? " · estimated API cost" : " · tokens"}
          </AppText>
          <AppText style={styles.hero}>{formatMeasure(total, metric)}</AppText>
          <View style={styles.facts}>
            <Fact label="Replies" value={shortCount(summary.total.replies)} />
            {metric === "cost" ? (
              <Fact label="Tokens" value={shortCount(summary.total.tokens)} />
            ) : (
              <Fact label="Est. cost" value={dollars(summary.total.cost)} />
            )}
            {summary.total.cacheSavings > 0 ? <Fact label="Saved by cache" value={dollars(summary.total.cacheSavings)} /> : null}
          </View>
          {metric === "tokens" && summary.total.tokens > 0 ? <TokenSplit totals={summary.total} /> : null}
          {metric === "cost" && summary.total.unpricedTokens > 0 ? (
            <AppText variant="caption" tone="textMuted">
              {`${shortCount(summary.total.unpricedTokens)} tokens are from models without a known price and aren't counted.`}
            </AppText>
          ) : null}
        </View>

        {/* Plan limits: the windows each account's plan allows. */}
        {claude.length + codex.length > 0 ? (
          <View style={styles.block}>
            <View style={styles.row}>
              <View style={styles.grow}>
                <SectionLabel>Plan limits</SectionLabel>
              </View>
              <Button
                label="Manage accounts"
                size="sm"
                variant="ghost"
                onPress={() => router.push(machineAccountsPath(machine.deviceId))}
              />
            </View>
            {[...claude, ...codex].map((account) => (
              <AccountCard
                key={`${account.provider}\u0000${account.account}`}
                account={account}
                multiple={(account.provider === "claude" ? claude : codex).length > 1}
                switching={switching === `${account.provider}\u0000${account.account}`}
                disabled={switching !== null || !machine.rpc}
                onUse={() => void useAccount(account)}
              />
            ))}
          </View>
        ) : null}

        {/* By day, stacked by agent. */}
        {summary.daily.length > 1 ? (
          <View style={styles.block}>
            <SectionLabel>By day</SectionLabel>
            <DailyChart summary={summary} metric={metric} colorOf={colorOf} />
          </View>
        ) : null}

        <Breakdown
          title="By agent"
          metric={metric}
          rows={summary.agents.map((entry) => ({
            key: entry.agent,
            label: agentName(entry.agent),
            totals: entry.totals,
            color: colorOf(entry.agent),
            agent: entry.agent,
          }))}
          empty="No agent has worked in this range."
        />
        <Breakdown
          title="By model"
          metric={metric}
          rows={summary.models.map((entry) => ({
            key: `${entry.agent}\u0000${entry.model}`,
            label: entry.model || "Unknown model",
            sublabel: agentName(entry.agent),
            totals: entry.totals,
            color: colorOf(entry.agent),
          }))}
        />
        <Breakdown
          title="By project"
          metric={metric}
          rows={summary.projects.map((entry) => ({
            key: entry.path,
            label: entry.name,
            totals: entry.totals,
            color: theme.colors.textMuted,
          }))}
        />
        <AppText variant="caption" tone="textSubtle">
          Read from each agent's own records on your desktop. Costs are estimates at API prices, not what a plan bills.
        </AppText>
      </ScrollView>
    </View>
  );
}

function SectionLabel({ children }: { children: string }) {
  return (
    <AppText variant="micro" tone="textMuted">
      {children.toUpperCase()}
    </AppText>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <View style={styles.fact}>
      <AppText style={styles.factValue}>{value}</AppText>
      <AppText variant="caption" tone="textMuted">{label}</AppText>
    </View>
  );
}

/** Input / output / cache, as one bar with a legend. */
function TokenSplit({ totals }: { totals: UsageTotals }) {
  const { theme } = useTheme();
  const parts = [
    { label: "Input", value: totals.input, color: theme.colors.accent },
    { label: "Output", value: totals.output, color: "#a78bfa" },
    { label: "Cache read", value: totals.cacheRead, color: theme.colors.success },
    { label: "Cache write", value: totals.cacheWrite, color: theme.colors.warning },
  ].filter((part) => part.value > 0);
  const sum = parts.reduce((all, part) => all + part.value, 0) || 1;
  return (
    <View style={styles.split}>
      <View style={[styles.splitBar, { backgroundColor: theme.colors.surfaceRaised }]}>
        {parts.map((part) => (
          <View key={part.label} style={{ flex: part.value / sum, backgroundColor: part.color }} />
        ))}
      </View>
      <View style={styles.legend}>
        {parts.map((part) => (
          <View key={part.label} style={styles.legendItem}>
            <View style={[styles.dot, { backgroundColor: part.color }]} />
            <AppText variant="caption" tone="textMuted">{`${part.label} ${shortCount(part.value)}`}</AppText>
          </View>
        ))}
      </View>
    </View>
  );
}

function AccountCard({
  account,
  multiple,
  switching,
  disabled,
  onUse,
}: {
  account: UsageAccount;
  multiple: boolean;
  switching: boolean;
  disabled: boolean;
  onUse: () => void;
}) {
  const { theme } = useTheme();
  const now = Date.now();
  return (
    <View style={[styles.card, { backgroundColor: theme.colors.surface, borderColor: account.inUse && multiple ? theme.colors.accent : theme.colors.border }]}>
      <View style={styles.row}>
        <AgentMark agent={account.provider} size={22} />
        <View style={styles.grow}>
          <AppText style={styles.strong} numberOfLines={1}>{agentName(account.provider)}</AppText>
          <AppText variant="caption" tone="textMuted" numberOfLines={1}>
            {[account.label, account.plan ? account.plan.charAt(0).toUpperCase() + account.plan.slice(1) : null].filter(Boolean).join(" · ")}
          </AppText>
        </View>
        {multiple && account.inUse ? <StatusBadge label="In use" tone="accent" /> : null}
      </View>
      {account.windows.length === 0 ? (
        <AppText variant="caption" tone="textMuted">
          {account.updatedAt === null
            ? "No reading yet: limits show after this account's next session."
            : "This plan reported no limits."}
        </AppText>
      ) : (
        account.windows.map((window) => {
          const tone = limitTone(window.usedPercent);
          const color = theme.colors[tone];
          const reset = resetsIn(window.resetsAt, now);
          return (
            <View key={window.label} style={styles.limit}>
              <View style={styles.row}>
                <AppText style={styles.grow}>{window.label}</AppText>
                <AppText style={[styles.strong, { color }]}>{`${Math.round(window.usedPercent)}%`}</AppText>
              </View>
              <View style={[styles.meter, { backgroundColor: theme.colors.surfaceRaised }]}>
                <View style={[styles.meterFill, { width: `${Math.max(2, window.usedPercent)}%`, backgroundColor: color }]} />
              </View>
              <AppText variant="caption" tone="textMuted">
                {`${Math.max(0, 100 - Math.round(window.usedPercent))}% left${reset ? ` · resets ${reset}` : ""}`}
              </AppText>
            </View>
          );
        })
      )}
      {account.updatedAt !== null && account.windows.length > 0 ? (
        <AppText variant="caption" tone="textSubtle">{`Read ${resetsAgo(account.updatedAt, now)}`}</AppText>
      ) : null}
      {multiple && !account.inUse ? (
        <View style={styles.rowEnd}>
          <Button label="Use for new sessions" size="sm" variant="ghost" loading={switching} disabled={disabled} onPress={onUse} />
        </View>
      ) : null}
    </View>
  );
}

function resetsAgo(updatedAt: number, now: number): string {
  const minutes = Math.max(0, Math.floor((now / 1000 - updatedAt) / 60));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}

/** One column per day, stacked by agent, newest on the right. */
function DailyChart({ summary, metric, colorOf }: { summary: UsageSummary; metric: UsageMetric; colorOf: (agent: string) => string }) {
  const { theme } = useTheme();
  const values = summary.daily.map((day) => Object.values(day.agents).reduce((sum, totals) => sum + measure(totals, metric), 0));
  const peak = Math.max(...values, 0);
  const [picked, setPicked] = useState<number | null>(null);
  const shown = picked !== null && summary.daily[picked] ? picked : null;
  const label = (date: string) => {
    const parsed = new Date(`${date}T12:00:00`);
    return parsed.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  };
  return (
    <View style={[styles.card, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
      <AppText variant="caption" tone="textMuted">
        {shown !== null
          ? `${label(summary.daily[shown]!.date)} · ${formatMeasure(values[shown]!, metric)}`
          : `Peak ${formatMeasure(peak, metric)} · tap a day`}
      </AppText>
      <View style={styles.chart}>
        {summary.daily.map((day, index) => {
          const agents = Object.entries(day.agents).filter(([, totals]) => measure(totals, metric) > 0);
          return (
            <Control
              key={day.date}
              style={styles.column}
              accessibilityRole="button"
              accessibilityLabel={`${label(day.date)}: ${formatMeasure(values[index]!, metric)}`}
              onPress={() => setPicked(index === picked ? null : index)}
            >
              <View style={[styles.stack, { opacity: shown === null || shown === index ? 1 : 0.35 }]}>
                {agents.map(([agent, totals]) => (
                  <View
                    key={agent}
                    style={{ height: peak > 0 ? (measure(totals, metric) / peak) * CHART_HEIGHT : 0, backgroundColor: colorOf(agent) }}
                  />
                ))}
                {agents.length === 0 ? <View style={[styles.none, { backgroundColor: theme.colors.surfaceRaised }]} /> : null}
              </View>
            </Control>
          );
        })}
      </View>
      <View style={styles.row}>
        <AppText variant="caption" tone="textSubtle" style={styles.grow}>{summary.daily[0] ? label(summary.daily[0].date) : ""}</AppText>
        <AppText variant="caption" tone="textSubtle">{summary.daily.at(-1) ? label(summary.daily.at(-1)!.date) : ""}</AppText>
      </View>
      {shown !== null ? (
        <View style={styles.legend}>
          {Object.entries(summary.daily[shown]!.agents)
            .filter(([, totals]) => measure(totals, metric) > 0)
            .map(([agent, totals]) => (
              <View key={agent} style={styles.legendItem}>
                <View style={[styles.dot, { backgroundColor: colorOf(agent) }]} />
                <AppText variant="caption" tone="textMuted">{`${agentName(agent)} ${formatMeasure(measure(totals, metric), metric)}`}</AppText>
              </View>
            ))}
        </View>
      ) : null}
    </View>
  );
}

const CHART_HEIGHT = 120;

interface BreakdownRow {
  key: string;
  label: string;
  sublabel?: string;
  totals: UsageTotals;
  color: string;
  agent?: string;
}

/** A ranked list with a share bar per row; long lists fold. */
function Breakdown({ title, metric, rows, empty }: { title: string; metric: UsageMetric; rows: BreakdownRow[]; empty?: string }) {
  const { theme } = useTheme();
  const [open, setOpen] = useState(false);
  const sorted = [...rows].sort((a, b) => measure(b.totals, metric) - measure(a.totals, metric));
  const top = Math.max(...sorted.map((row) => measure(row.totals, metric)), 0);
  if (sorted.length === 0 && !empty) return null;
  const shown = open ? sorted : sorted.slice(0, FOLDED);
  return (
    <View style={styles.block}>
      <SectionLabel>{title}</SectionLabel>
      <View style={[styles.card, styles.list, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
        {sorted.length === 0 ? <AppText tone="textMuted">{empty}</AppText> : null}
        {shown.map((row) => {
          const value = measure(row.totals, metric);
          return (
            <View key={row.key} style={styles.entry}>
              <View style={styles.row}>
                {row.agent ? <AgentMark agent={row.agent} size={16} /> : <View style={[styles.dot, { backgroundColor: row.color }]} />}
                <View style={styles.grow}>
                  <AppText numberOfLines={1}>{row.label}</AppText>
                  {row.sublabel ? <AppText variant="caption" tone="textMuted" numberOfLines={1}>{row.sublabel}</AppText> : null}
                </View>
                <AppText style={styles.strong}>{formatMeasure(value, metric)}</AppText>
              </View>
              <View style={[styles.share, { backgroundColor: theme.colors.surfaceRaised }]}>
                <View style={[styles.shareFill, { width: `${top > 0 ? Math.max(1, (value / top) * 100) : 0}%`, backgroundColor: row.color }]} />
              </View>
            </View>
          );
        })}
        {sorted.length > FOLDED ? (
          <Button
            label={open ? "Show fewer" : `Show all ${sorted.length}`}
            size="sm"
            variant="ghost"
            onPress={() => setOpen((value) => !value)}
          />
        ) : null}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  controls: { gap: spacing.sm, paddingVertical: spacing.md },
  content: { gap: spacing.lg, paddingBottom: spacing.xl, paddingTop: spacing.xs },
  padded: { gap: spacing.md, paddingVertical: spacing.md },
  row: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  rowEnd: { flexDirection: "row", justifyContent: "flex-end" },
  grow: { flex: 1, minWidth: 0 },
  strong: { fontWeight: "600" },
  block: { gap: spacing.sm },
  card: { borderWidth: 1, borderRadius: radii.xl, borderCurve: "continuous", padding: spacing.md, gap: spacing.sm },
  list: { gap: spacing.md },
  hero: { fontSize: 34, lineHeight: 40, fontWeight: "700", letterSpacing: -0.5 },
  facts: { flexDirection: "row", flexWrap: "wrap", gap: spacing.lg },
  fact: { gap: 0 },
  factValue: { fontWeight: "600" },
  split: { gap: spacing.xs, paddingTop: spacing.xs },
  splitBar: { height: 8, borderRadius: 4, overflow: "hidden", flexDirection: "row" },
  legend: { flexDirection: "row", flexWrap: "wrap", gap: spacing.md },
  legendItem: { flexDirection: "row", alignItems: "center", gap: spacing.xs },
  dot: { width: 8, height: 8, borderRadius: 4 },
  limit: { gap: spacing.xs },
  meter: { height: 10, borderRadius: 5, overflow: "hidden" },
  meterFill: { height: 10, borderRadius: 5 },
  chart: { height: CHART_HEIGHT, flexDirection: "row", alignItems: "flex-end", gap: 3 },
  column: { flex: 1, height: CHART_HEIGHT, justifyContent: "flex-end" },
  stack: { borderRadius: 3, overflow: "hidden", flexDirection: "column-reverse" },
  none: { height: 2 },
  entry: { gap: spacing.xs },
  share: { height: 4, borderRadius: 2, overflow: "hidden" },
  shareFill: { height: 4, borderRadius: 2 },
});
