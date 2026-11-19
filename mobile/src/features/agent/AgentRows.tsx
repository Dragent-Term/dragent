import { memo, useState, type ReactNode } from "react";
import { ActivityIndicator, Platform, ScrollView, StyleSheet, Text, View } from "react-native";
import { AppText, Button, Control, Icon } from "../../components";
import { FieldInput } from "../../components/FieldInput";
import { statusMarkColors, useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { agentDurationLabel, type AgentRow } from "../../transport/agentConversation";
import type { AgentAction } from "../../transport/relayRpcMethods";
import { Markdown } from "./AgentMarkdown";
import { ReplyLine, YouLine } from "../chat/TranscriptParts";
import type { AgentDelivery } from "./useAgentConversation";

const MONO = Platform.select({ ios: "Menlo", android: "monospace", default: "monospace" });

export type AgentAct = (
  action: AgentAction,
  extra?: { ref?: string | null; option?: string; text?: string },
) => Promise<AgentDelivery>;

type RowOf<K extends AgentRow["kind"]> = Extract<AgentRow, { kind: K }>;

/** Runs one card action at a time and keeps its failure next to the card. */
function useCardAction(act: AgentAct) {
  const [running, setRunning] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = (key: string, action: AgentAction, extra?: Parameters<AgentAct>[1]) => {
    if (running !== null) return;
    setRunning(key);
    setError(null);
    void act(action, extra).then((result) => {
      setRunning(null);
      if (!result.ok) setError(result.message);
    });
  };
  return { running, error, run };
}

function Card({ children, tone }: { children: ReactNode; tone?: string }) {
  const { theme } = useTheme();
  return (
    <View
      style={[
        styles.card,
        { backgroundColor: theme.colors.surfaceRaised, borderColor: tone ?? theme.colors.border },
      ]}
    >
      {children}
    </View>
  );
}

function CardError({ message }: { message: string | null }) {
  if (!message) return null;
  return <AppText variant="caption" tone="danger" accessibilityRole="alert">{message}</AppText>;
}

function Mono({ children, muted = false }: { children: string; muted?: boolean }) {
  const { theme } = useTheme();
  return (
    <Text selectable style={[styles.mono, { color: muted ? theme.colors.textMuted : theme.colors.text }]}>
      {children}
    </Text>
  );
}

const UserQuery = memo(function UserQuery({ row }: { row: RowOf<"user_query"> }) {
  return <YouLine text={row.text} note={null} />;
});

const AgentMessage = memo(function AgentMessage({ row, last }: { row: RowOf<"agent_message">; last: boolean }) {
  return <ReplyLine text={row.text} lastReply={last && !row.streaming} at={null} />;
});

const Thinking = memo(function Thinking({ row }: { row: RowOf<"thinking"> }) {
  const { theme } = useTheme();
  const [open, setOpen] = useState(false);
  return (
    <View style={styles.plain}>
      <Control
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        accessibilityLabel={row.complete ? "Thought" : "Thinking"}
        onPress={() => setOpen((value) => !value)}
        style={styles.disclosure}
      >
        {row.complete ? null : <ActivityIndicator size="small" color={theme.colors.textMuted} />}
        <AppText variant="caption" tone="textMuted" style={styles.strong}>{row.complete ? "Thought" : "Thinking…"}</AppText>
        <Icon name={open ? "chevron-down" : "chevron-right"} size={14} tone="textSubtle" />
      </Control>
      {open && row.text ? (
        <View style={[styles.indent, { borderColor: theme.colors.border }]}>
          <Markdown text={row.text} muted />
        </View>
      ) : null}
    </View>
  );
});

function DiffFiles({ row }: { row: RowOf<"tool_step"> }) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  return (
    <>
      {row.diff.map((file) => (
        <View key={file.path} style={[styles.diff, { borderColor: theme.colors.border, backgroundColor: theme.colors.background }]}>
          <View style={styles.diffHead}>
            <AppText variant="caption" numberOfLines={1} style={[styles.flex, styles.strong]}>{file.path}</AppText>
            <AppText variant="caption" style={{ color: marks.success }}>+{file.added}</AppText>
            <AppText variant="caption" style={{ color: marks.danger }}>−{file.removed}</AppText>
          </View>
          <ScrollView horizontal showsHorizontalScrollIndicator={false}>
            <View>
              {file.lines.map((line, index) => (
                <Text
                  key={index}
                  style={[
                    styles.mono,
                    {
                      color: line.kind === "add" ? marks.success : line.kind === "del" ? marks.danger : theme.colors.textMuted,
                    },
                  ]}
                >
                  {line.kind === "add" ? "+ " : line.kind === "del" ? "− " : "  "}
                  {line.text}
                </Text>
              ))}
            </View>
          </ScrollView>
        </View>
      ))}
    </>
  );
}

const ToolStep = memo(function ToolStep({ row }: { row: RowOf<"tool_step"> }) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  const [open, setOpen] = useState(false);
  const duration = agentDurationLabel(row.durationMs);
  const hasDetail = row.result !== null || row.diff.length > 0 || row.args.length > 60;
  const added = row.diff.reduce((sum, file) => sum + file.added, 0);
  const removed = row.diff.reduce((sum, file) => sum + file.removed, 0);
  return (
    <View style={styles.plain}>
      <Control
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        accessibilityLabel={`${row.name} ${row.args}, ${row.status}`}
        disabled={!hasDetail}
        onPress={() => setOpen((value) => !value)}
        style={styles.tool}
      >
        <View style={styles.toolGlyph}>
          {row.status === "running" ? (
            <ActivityIndicator size="small" color={theme.colors.accent} />
          ) : row.status === "error" ? (
            <Icon name="close" size={14} tone="danger" />
          ) : (
            <Icon name="check" size={14} tone="textSubtle" />
          )}
        </View>
        <Text numberOfLines={open ? undefined : 1} style={[styles.toolLine, { color: theme.colors.textMuted }]}>
          <Text style={[styles.strong, { color: row.status === "error" ? marks.danger : theme.colors.text }]}>{row.name}</Text>
          {row.args ? `  ${row.args}` : ""}
        </Text>
        {row.diff.length > 0 ? (
          <>
            <AppText variant="caption" style={{ color: marks.success }}>+{added}</AppText>
            <AppText variant="caption" style={{ color: marks.danger }}>−{removed}</AppText>
          </>
        ) : duration && row.status !== "running" ? (
          <AppText variant="caption" tone="textSubtle">{duration}</AppText>
        ) : null}
        {hasDetail ? <Icon name={open ? "chevron-down" : "chevron-right"} size={14} tone="textSubtle" /> : null}
      </Control>
      {open ? (
        <View style={[styles.indent, { borderColor: theme.colors.border }]}>
          {row.result ? <Mono muted>{row.result}</Mono> : null}
          <DiffFiles row={row} />
        </View>
      ) : null}
    </View>
  );
});

const ActionResult = memo(function ActionResult({ row }: { row: RowOf<"action_result"> }) {
  const { theme } = useTheme();
  const [open, setOpen] = useState(false);
  const failed = row.exitCode !== null && row.exitCode !== 0;
  return (
    <View style={styles.plain}>
      <Control
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        accessibilityLabel={`Command ${row.command}`}
        disabled={!row.output}
        onPress={() => setOpen((value) => !value)}
        style={styles.tool}
      >
        <View style={styles.toolGlyph}>
          <Icon name={failed ? "close" : "check"} size={14} tone={failed ? "danger" : "textSubtle"} />
        </View>
        <Text numberOfLines={open ? undefined : 1} style={[styles.toolLine, styles.monoInline, { color: theme.colors.text }]}>
          {row.command}
        </Text>
        {failed ? <AppText variant="caption" tone="danger">exit {row.exitCode}</AppText> : null}
        {row.output ? <Icon name={open ? "chevron-down" : "chevron-right"} size={14} tone="textSubtle" /> : null}
      </Control>
      {open ? (
        <View style={[styles.indent, { borderColor: theme.colors.border }]}>
          <Mono muted>{row.output}</Mono>
        </View>
      ) : null}
    </View>
  );
});

const APPROVAL_DONE: Record<string, string> = {
  approved: "Approved",
  approved_always: "Always allowed",
  rejected: "Rejected",
};

const Approval = memo(function Approval({ row, act }: { row: RowOf<"approval">; act: AgentAct }) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  const { running, error, run } = useCardAction(act);
  const pending = row.status === "pending";
  return (
    <Card {...(pending ? { tone: marks.warning } : {})}>
      <AppText variant="caption" style={[styles.strong, { color: pending ? marks.warning : theme.colors.textMuted }]}>
        {pending ? "Needs your approval" : APPROVAL_DONE[row.status] ?? "Answered"}
      </AppText>
      <Mono>{row.command}</Mono>
      {row.reason ? <AppText variant="caption" tone="textMuted">{row.reason}</AppText> : null}
      {pending && row.ref ? (
        <View style={styles.actions}>
          <Button label="Approve" size="sm" loading={running === "approve"} disabled={running !== null}
            onPress={() => run("approve", "approve", { ref: row.ref })} />
          <Button label="Always allow" size="sm" variant="ghost" loading={running === "always"} disabled={running !== null}
            onPress={() => run("always", "approve_always", { ref: row.ref })} />
          <Button label="Reject" size="sm" variant="danger" loading={running === "reject"} disabled={running !== null}
            onPress={() => run("reject", "reject", { ref: row.ref })} />
        </View>
      ) : null}
      <CardError message={error} />
    </Card>
  );
});

const Question = memo(function Question({ row, act }: { row: RowOf<"question">; act: AgentAct }) {
  const { theme } = useTheme();
  const { running, error, run } = useCardAction(act);
  const [other, setOther] = useState("");
  const pending = row.status === "pending" && row.ref !== null;
  return (
    <Card {...(pending ? { tone: theme.colors.accent } : {})}>
      <View style={styles.questionHead}>
        <AppText variant="caption" tone={pending ? "accent" : "textMuted"} style={styles.strong}>
          {row.header || (pending ? "Dragent is asking" : row.status === "answered" ? "Answered" : "Dismissed")}
        </AppText>
        {row.count > 1 ? (
          <AppText variant="caption" tone="textSubtle">{row.index + 1} of {row.count}</AppText>
        ) : null}
      </View>
      <AppText variant="body" selectable>{row.question}</AppText>
      {pending ? (
        <>
          {row.options.map((option) => {
            const picked = row.picked.includes(option.label);
            return (
              <Control
                key={option.label}
                accessibilityRole="button"
                accessibilityState={{ selected: picked }}
                accessibilityLabel={option.description ? `${option.label}. ${option.description}` : option.label}
                disabled={running !== null}
                onPress={() => run(`option:${option.label}`, "answer", { ref: row.ref, option: option.label })}
                style={({ pressed }) => [
                  styles.option,
                  {
                    borderColor: picked ? theme.colors.accent : theme.colors.border,
                    backgroundColor: pressed || picked ? theme.colors.background : theme.colors.surface,
                  },
                ]}
              >
                <View style={styles.flex}>
                  <AppText variant="body" style={styles.strong}>{option.label}</AppText>
                  {option.description ? <AppText variant="caption" tone="textMuted">{option.description}</AppText> : null}
                </View>
                {running === `option:${option.label}` ? (
                  <ActivityIndicator size="small" color={theme.colors.accent} />
                ) : picked ? (
                  <Icon name="check" size={16} tone="accent" />
                ) : null}
              </Control>
            );
          })}
          {row.multi ? (
            <Button label="Confirm choices" size="sm" loading={running === "confirm"}
              disabled={running !== null || row.picked.length === 0}
              onPress={() => run("confirm", "confirm_answer", { ref: row.ref })} />
          ) : null}
          {row.custom ? (
            <View style={styles.otherRow}>
              <FieldInput containerStyle={styles.flex} style={styles.otherInput} value={other} onChangeText={setOther}
                placeholder="Something else…" accessibilityLabel="Your own answer" multiline />
              <Button label="Send" size="sm" variant="ghost" loading={running === "text"}
                disabled={running !== null || other.trim() === ""}
                onPress={() => run("text", "answer_text", { ref: row.ref, text: other.trim() })} />
            </View>
          ) : null}
        </>
      ) : row.picked.length > 0 ? (
        <AppText variant="caption" tone="textMuted">{row.picked.join(", ")}</AppText>
      ) : null}
      <CardError message={error} />
    </Card>
  );
});

const PlanReview = memo(function PlanReview({ row, act }: { row: RowOf<"plan_review">; act: AgentAct }) {
  const { theme } = useTheme();
  const { running, error, run } = useCardAction(act);
  const pending = row.status === "pending" && row.ref !== null;
  return (
    <Card {...(pending ? { tone: theme.colors.accent } : {})}>
      <AppText variant="caption" tone={pending ? "accent" : "textMuted"} style={styles.strong}>
        {pending ? "Plan to review" : row.status === "approved" ? "Plan approved" : "Refining the plan"}
      </AppText>
      <Markdown text={row.content} />
      {pending ? (
        <View style={styles.actions}>
          <Button label="Approve plan" size="sm" loading={running === "approve"} disabled={running !== null}
            onPress={() => run("approve", "approve_plan", { ref: row.ref })} />
          <Button label="Refine" size="sm" variant="ghost" loading={running === "refine"} disabled={running !== null}
            onPress={() => run("refine", "refine_plan", { ref: row.ref })} />
        </View>
      ) : null}
      {row.status === "refining" ? (
        <AppText variant="caption" tone="textMuted">Say what to change in the message box below.</AppText>
      ) : null}
      <CardError message={error} />
    </Card>
  );
});

const Queued = memo(function Queued({ row, act }: { row: RowOf<"queued">; act: AgentAct }) {
  const { error, run } = useCardAction(act);
  return (
    <>
      <YouLine
        text={row.text}
        note={{ kind: "queued" }}
        {...(row.ref ? { onCancel: () => run("cancel", "cancel_queued", { ref: row.ref }) } : {})}
      />
      <CardError message={error} />
    </>
  );
});

const TODO_MARK: Record<string, string> = { completed: "✓", in_progress: "◐", cancelled: "✕" };

const Todos = memo(function Todos({ row }: { row: RowOf<"todos"> }) {
  const { theme } = useTheme();
  return (
    <Card>
      <AppText variant="caption" tone="textMuted" style={styles.strong}>Tasks</AppText>
      {row.items.map((item, index) => {
        const done = item.status === "completed" || item.status === "cancelled";
        return (
          <View key={index} style={styles.todo}>
            <Text style={[styles.todoMark, { color: item.status === "in_progress" ? theme.colors.accent : theme.colors.textSubtle }]}>
              {TODO_MARK[item.status] ?? "○"}
            </Text>
            <AppText
              variant="body"
              tone={done ? "textMuted" : "text"}
              style={[styles.flex, done ? styles.struck : null]}
            >
              {item.content}
            </AppText>
          </View>
        );
      })}
    </Card>
  );
});

function Divider({ label }: { label: string }) {
  const { theme } = useTheme();
  return (
    <View style={styles.divider} accessibilityLabel={label}>
      <View style={[styles.rule, { backgroundColor: theme.colors.border }]} />
      <AppText variant="caption" tone="textSubtle">{label}</AppText>
      <View style={[styles.rule, { backgroundColor: theme.colors.border }]} />
    </View>
  );
}

/**
 * One row of the built-in agent's conversation. `last` marks the reply that
 * ends a run of the agent's rows (it gets Copy).
 */
export const AgentRowView = memo(function AgentRowView({ row, act, last = false }: { row: AgentRow; act: AgentAct; last?: boolean }) {
  const { theme } = useTheme();
  const body = (() => {
    switch (row.kind) {
      case "user_query": return <UserQuery row={row} />;
      case "agent_message": return <AgentMessage row={row} last={last} />;
      case "thinking": return <Thinking row={row} />;
      case "tool_step": return <ToolStep row={row} />;
      case "action_result": return <ActionResult row={row} />;
      case "approval": return <Approval row={row} act={act} />;
      case "question": return <Question row={row} act={act} />;
      case "plan_review": return <PlanReview row={row} act={act} />;
      case "queued": return <Queued row={row} act={act} />;
      case "todos": return <Todos row={row} />;
      case "system_note":
        return <AppText variant="caption" tone="textMuted" selectable>{row.text}</AppText>;
      case "loading":
        return (
          <View style={styles.loading}>
            <ActivityIndicator size="small" color={theme.colors.accent} />
            <AppText variant="caption" tone="textMuted">{row.message || "Working…"}</AppText>
          </View>
        );
      case "error":
        return (
          <Card tone={theme.colors.danger}>
            <AppText variant="caption" tone="danger" style={styles.strong}>Something went wrong</AppText>
            <AppText variant="body" selectable>{row.message}</AppText>
          </Card>
        );
      case "turn_end": {
        const length = agentDurationLabel(row.durationMs);
        return <Divider label={row.note ?? (length ? `Worked for ${length}` : "Done")} />;
      }
      default:
        return row.text ? <AppText variant="caption" tone="textMuted" selectable>{row.text}</AppText> : null;
    }
  })();
  if (body === null) return null;
  return (
    <View style={styles.row}>
      {body}
      {row.clipped ? (
        <AppText variant="caption" tone="textSubtle">Long content was shortened. The full text is on your desktop.</AppText>
      ) : null}
    </View>
  );
});

const styles = StyleSheet.create({
  row: { gap: spacing.xs, minWidth: 0 },
  plain: { minWidth: 0, gap: spacing.xs },
  flex: { flex: 1, minWidth: 0 },
  strong: { fontWeight: "600" },
  card: { borderWidth: StyleSheet.hairlineWidth, borderRadius: radii.lg, padding: spacing.md, gap: spacing.sm, minWidth: 0 },
  disclosure: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 32, alignSelf: "flex-start" },
  indent: { borderLeftWidth: 2, paddingLeft: spacing.sm, marginLeft: 6, gap: spacing.sm },
  tool: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 32 },
  toolGlyph: { width: 20, alignItems: "center" },
  toolLine: { flex: 1, minWidth: 0, fontSize: 13, lineHeight: 19 },
  mono: { fontFamily: MONO, fontSize: 12, lineHeight: 17 },
  monoInline: { fontFamily: MONO, fontSize: 12 },
  diff: { borderWidth: StyleSheet.hairlineWidth, borderRadius: radii.md, padding: spacing.sm, gap: spacing.xs },
  diffHead: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
  questionHead: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: spacing.sm },
  option: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: radii.md,
    paddingHorizontal: spacing.md,
    paddingVertical: spacing.sm,
    minHeight: 44,
  },
  otherRow: { flexDirection: "row", alignItems: "flex-end", gap: spacing.sm },
  otherInput: { minHeight: 40, maxHeight: 96 },
  todo: { flexDirection: "row", gap: spacing.sm, alignItems: "flex-start" },
  todoMark: { width: 16, fontSize: 14, lineHeight: 21, textAlign: "center" },
  struck: { textDecorationLine: "line-through" },
  loading: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  divider: { flexDirection: "row", alignItems: "center", gap: spacing.sm, paddingVertical: 2 },
  rule: { flex: 1, height: StyleSheet.hairlineWidth },
});
