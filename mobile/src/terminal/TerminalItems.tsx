import { memo, useState } from "react";
import { Platform, ScrollView, StyleSheet, View } from "react-native";
import { AppText, AgentStateDot, agentStateLabel, Control, StatusBadge, Surface, type StatusTone, type TextTone } from "../components";
import { useTheme } from "../theme";
import { radii, spacing } from "../theme/tokens";
import { agentDotBadgeTone, agentDotState, agentRole, defaultAgentExpanded, isAgentCollapsible, spanPresentation } from "./presentation";
import type { TerminalAgentBlock, TerminalAltScreen, TerminalBlock, TerminalLine } from "./types";

/** Host-rendered snapshot note. Input controls live outside this viewer. */
export const altScreenNote = "Full-screen programs are shown as host-rendered snapshots.";
export const rawPendingNote = "The host sent raw terminal data. Waiting for a styled snapshot.";

export function ClippedOutput() {
  return <AppText variant="caption" tone="textSubtle">Earlier rows or long content were clipped to keep this view responsive.</AppText>;
}

/** One horizontal scroller per block keeps its rows aligned without wrapping. */
type TerminalPresentation = "cards" | "canvas";

export const TerminalRows = memo(function TerminalRows({ lines, presentation = "cards" }: { lines: readonly TerminalLine[]; presentation?: TerminalPresentation }) {
  const { theme } = useTheme();
  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator
      keyboardShouldPersistTaps="handled"
      style={[styles.output, presentation === "canvas" ? styles.flatOutput : null, { backgroundColor: theme.terminal.background }]}
      contentContainerStyle={presentation === "canvas" ? undefined : styles.horizontalContent}
    >
      <View style={styles.rows}>
        {lines.map((line) => (
          <AppText key={line.index} variant="caption" selectable style={styles.row}>
            {line.spans.length === 0 ? " " : line.spans.map((span, index) => {
              const rendered = spanPresentation(span, theme.terminal);
              return (
                <AppText key={index} variant="caption" style={[styles.monospace, rendered.style]}>
                  {rendered.text}
                </AppText>
              );
            })}
          </AppText>
        ))}
      </View>
    </ScrollView>
  );
});

export const CommandBlock = memo(function CommandBlock({ block, presentation = "cards" }: { block: TerminalBlock; presentation?: TerminalPresentation }) {
  const tone: StatusTone = block.status === "running" ? "accent"
    : block.failed ? "danger" : block.exitCode === 0 ? "success" : "neutral";
  const label = block.status === "running" ? "Running"
    : block.failed ? "Failed" : block.exitCode === null ? "Completed" : `Exit ${block.exitCode}`;
  return (
    <Surface
      appearance={presentation === "canvas" ? "plain" : "card"}
      header={presentation === "canvas" ? (
        <AppText variant="caption" tone={block.failed ? "danger" : "text"} selectable style={styles.monospace}>
          {`$ ${block.command}`}{block.failed ? ` · ${label}` : ""}
        </AppText>
      ) : (
        <View style={styles.blockHeader}>
          <View style={styles.blockTitles}>
            <AppText selectable style={styles.monospace}>{block.command || "Command output"}</AppText>
            {block.cwd ? <AppText variant="caption" tone="textMuted" selectable>{block.cwd}</AppText> : null}
          </View>
          <StatusBadge label={label} tone={tone} />
        </View>
      )}
      footer={presentation === "canvas" || block.durationMs === null ? null : (
        <AppText variant="caption" tone="textSubtle">{`Duration: ${block.durationMs} ms`}</AppText>
      )}
    >
      {block.truncated ? <ClippedOutput /> : null}
      {block.lines.length > 0 ? <TerminalRows lines={block.lines} presentation={presentation} /> : (
        <AppText variant="caption" tone="textMuted">
          {block.status === "running" ? "Waiting for styled output…" : "No styled output was shared for this command."}
        </AppText>
      )}
    </Surface>
  );
});

function agentLabelTone(role: ReturnType<typeof agentRole>): TextTone {
  switch (role) {
    case "user": return "accent";
    case "agent": return "text";
    case "request": return "warning";
    default: return "textMuted";
  }
}

function agentBodyTone(role: ReturnType<typeof agentRole>): TextTone {
  switch (role) {
    case "user":
    case "agent":
    case "request":
    case "group":
    case "unknown": return "text";
    default: return "textMuted";
  }
}

function agentBadgeTone(block: TerminalAgentBlock): StatusTone {
  return agentDotBadgeTone(agentDotState(block));
}

function disclosureCopy(kind: TerminalAgentBlock["kind"]): { show: string; hide: string } {
  switch (kind) {
    case "thinking": return { show: "Show thinking", hide: "Hide thinking" };
    case "tool_step": return { show: "Show tool details", hide: "Hide tool details" };
    case "turn_group":
    case "activity_group": return { show: "Show grouped conversation", hide: "Hide grouped conversation" };
    default: return { show: "Show details", hide: "Hide details" };
  }
}

export interface AgentBlockProps {
  block: TerminalAgentBlock;
  presentation?: TerminalPresentation;
  /** Controlled expand state; falls back to collapsed-by-default verbose rows. */
  expanded?: boolean;
  onToggle?: () => void;
}

export const AgentBlock = memo(function AgentBlock({ block, presentation = "cards", expanded: controlled, onToggle }: AgentBlockProps) {
  const role = agentRole(block.kind);
  const collapsible = isAgentCollapsible(block.kind);
  const [internal, setInternal] = useState(() => defaultAgentExpanded(block.kind));
  const expanded = controlled ?? internal;
  const toggle = onToggle ?? (() => setInternal((value) => !value));
  const copy = disclosureCopy(block.kind);
  const isMonospace = role === "tool" || role === "result";
  const isThought = role === "thought";
  const bodyVariant = role === "user" || role === "agent" || role === "request" || role === "group" || role === "unknown" || block.kind === "error"
    ? "body" as const : "caption" as const;
  // One AgentStateDot vocabulary everywhere: stop/retry/interrupted read as
  // words even when the host sent no status string.
  const dot = agentDotState(block);
  const badge = block.status ? (
    <StatusBadge label={block.status} tone={agentBadgeTone(block)} accessibilityLabel={`${agentStateLabel(dot)}: ${block.status}`} />
  ) : dot !== "idle" ? (
    <StatusBadge label={agentStateLabel(dot)} tone={agentDotBadgeTone(dot)} />
  ) : null;

  return (
    <Surface
      appearance={presentation === "canvas" ? "plain" : "card"}
      header={
        <View style={styles.blockHeader}>
          <View style={styles.dot} accessible={false} aria-hidden accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
            <AgentStateDot state={dot} />
          </View>
          <View style={styles.blockTitles}>
            <AppText variant="caption" tone={agentLabelTone(role)} style={styles.agentLabel}>
              {block.label}
            </AppText>
            {badge}
          </View>
        </View>
      }
    >
      {collapsible ? (
        <Control
          accessibilityRole="button"
          accessibilityState={{ expanded }}
          accessibilityLabel={expanded ? `${block.label}: ${copy.hide}` : `${block.label}: ${copy.show}`}
          accessibilityHint={expanded ? "Collapses this section." : "Expands this section to read the full text."}
          onPress={toggle}
          style={styles.disclosure}
        >
          <AppText variant="caption" tone="accent" style={styles.disclosureLabel}>
            {expanded ? copy.hide : copy.show}
          </AppText>
        </Control>
      ) : null}
      {!collapsible || expanded ? (
        block.text ? (
          <AppText
            variant={bodyVariant}
            tone={agentBodyTone(block.kind === "error" ? "agent" : role)}
            selectable
            style={[styles.bodyText, isMonospace ? styles.monospace : null, isThought ? styles.thought : null, block.kind === "error" && role === "agent" ? styles.errorBody : null]}
          >
            {block.text}
          </AppText>
        ) : (
          <AppText variant="caption" tone="textMuted" selectable style={styles.bodyText}>
            No text was shared for this update.
          </AppText>
        )
      ) : null}
      {block.truncated ? <ClippedOutput /> : null}
    </Surface>
  );
});

export const AltScreenBlock = memo(function AltScreenBlock({ screen, presentation = "cards" }: { screen: TerminalAltScreen; presentation?: TerminalPresentation }) {
  return (
    <Surface appearance={presentation === "canvas" ? "plain" : "card"} header={<StatusBadge label="Full-screen snapshot" tone="accent" />}>
      <AppText variant="caption" tone="textMuted">{altScreenNote}</AppText>
      <AppText variant="caption" tone="textSubtle">
        {`${screen.rows} rows × ${screen.columns} columns${screen.cursorVisible ? ` · Cursor at row ${screen.cursorRow + 1}, column ${screen.cursorColumn + 1}` : ""}`}
      </AppText>
      {screen.truncated ? <ClippedOutput /> : null}
      <TerminalRows lines={screen.lines} presentation={presentation} />
    </Surface>
  );
});

const styles = StyleSheet.create({
  // Fluid wrapping row: dot stays fixed while titles shrink and wrap, so 320px
  // phones and large Dynamic Type sizes stack instead of overflowing. Only
  // code blocks (TerminalRows) scroll horizontally; all prose wraps.
  blockHeader: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.md, minWidth: 0, maxWidth: "100%" },
  blockTitles: { flexGrow: 1, flexShrink: 1, flexBasis: 0, minWidth: 0, maxWidth: "100%", gap: spacing.xs },
  dot: { flexShrink: 0 },
  agentLabel: { flexShrink: 1, maxWidth: "100%", fontWeight: "600" },
  bodyText: { flexShrink: 1, maxWidth: "100%" },
  output: { borderRadius: radii.sm, borderCurve: "continuous", maxWidth: "100%" },
  flatOutput: { borderRadius: 0 },
  horizontalContent: { padding: spacing.md },
  rows: { flexShrink: 0 },
  row: { flexShrink: 0, ...monospaceStyle() },
  monospace: monospaceStyle(),
  thought: { fontStyle: "italic" },
  errorBody: { fontWeight: "400" },
  disclosure: { alignSelf: "flex-start", minHeight: 48, justifyContent: "center", paddingVertical: spacing.xs, minWidth: 48, maxWidth: "100%" },
  disclosureLabel: { fontWeight: "600", flexShrink: 1 },
});

function monospaceStyle() {
  return { fontFamily: Platform.OS === "ios" ? "Menlo" : "monospace" };
}
