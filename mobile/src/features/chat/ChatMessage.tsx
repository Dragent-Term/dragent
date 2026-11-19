import { memo } from "react";
import { StyleSheet, View } from "react-native";
import { AppText, StatusBadge, Surface, type StatusTone } from "../../components";
import { agentRole } from "../../terminal";
import type { TerminalAgentBlock } from "../../terminal";
import { spacing } from "../../theme/tokens";
import { ChatClippedNote, ChatDisclosure } from "./ChatDisclosure";

function badgeTone(block: TerminalAgentBlock): StatusTone {
  if (block.kind === "error") return "danger";
  if (agentRole(block.kind) === "request") return "warning";
  if (block.status === "Streaming" || block.kind === "loading" || block.kind === "queued") return "accent";
  return "neutral";
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

const UserMessage = memo(function UserMessage({ block }: { block: TerminalAgentBlock }) {
  return (
    <Surface tone="surfaceRaised">
      <View style={styles.row}>
        <AppText variant="caption" tone="accent" style={styles.label}>{block.label}</AppText>
        {block.status ? <StatusBadge label={block.status} tone={badgeTone(block)} /> : null}
      </View>
      {block.text ? (
        <AppText variant="body" selectable>{block.text}</AppText>
      ) : (
        <AppText variant="caption" tone="textMuted">No text was shared for this message.</AppText>
      )}
      {block.truncated ? <ChatClippedNote /> : null}
    </Surface>
  );
});

const AssistantMessage = memo(function AssistantMessage({ block }: { block: TerminalAgentBlock }) {
  return (
    <View style={styles.plain}>
      <View style={styles.row}>
        <AppText variant="caption" tone="textMuted" style={styles.label}>{block.label}</AppText>
        {block.status ? <StatusBadge label={block.status} tone={badgeTone(block)} /> : null}
      </View>
      {block.text ? (
        <AppText variant="body" selectable>{block.text}</AppText>
      ) : (
        <AppText variant="caption" tone="textMuted">No text was shared for this update.</AppText>
      )}
      {block.truncated ? <ChatClippedNote /> : null}
    </View>
  );
});

const QuietNote = memo(function QuietNote({ block }: { block: TerminalAgentBlock }) {
  return (
    <View style={styles.plain}>
      <View style={styles.row}>
        <AppText variant="caption" tone="textSubtle" style={styles.label}>{block.label}</AppText>
        {block.status ? <StatusBadge label={block.status} tone={badgeTone(block)} /> : null}
      </View>
      {block.text ? (
        <AppText variant="caption" tone="textMuted" selectable>{block.text}</AppText>
      ) : null}
      {block.truncated ? <ChatClippedNote /> : null}
    </View>
  );
});

const CollapsibleMessage = memo(function CollapsibleMessage({ block }: { block: TerminalAgentBlock }) {
  const copy = disclosureCopy(block.kind);
  const thought = agentRole(block.kind) === "thought";
  return (
    <View style={styles.plain}>
      <View style={styles.row}>
        <AppText variant="caption" tone="textMuted" style={styles.label}>{block.label}</AppText>
        {block.status ? <StatusBadge label={block.status} tone={badgeTone(block)} /> : null}
      </View>
      <ChatDisclosure label={block.label} showLabel={copy.show} hideLabel={copy.hide}>
        {block.text ? (
          <AppText variant="caption" tone="textMuted" selectable style={thought ? styles.thought : null}>
            {block.text}
          </AppText>
        ) : (
          <AppText variant="caption" tone="textMuted">No text was shared for this update.</AppText>
        )}
      </ChatDisclosure>
      {block.truncated ? <ChatClippedNote /> : null}
    </View>
  );
});

/**
 * Approval/question/plan_review rows are display-only. The wire exposes no
 * response commands, so no action buttons are rendered here.
 */
const ReadOnlyRequest = memo(function ReadOnlyRequest({ block }: { block: TerminalAgentBlock }) {
  return (
    <Surface>
      <View style={styles.row}>
        <AppText variant="caption" tone="warning" style={styles.label}>{block.label}</AppText>
        {block.status ? <StatusBadge label={block.status} tone={badgeTone(block)} /> : null}
      </View>
      {block.text ? (
        <AppText variant="body" selectable>{block.text}</AppText>
      ) : (
        <AppText variant="caption" tone="textMuted">No text was shared for this request.</AppText>
      )}
      <AppText variant="caption" tone="textSubtle">Read only — respond from the host.</AppText>
      {block.truncated ? <ChatClippedNote /> : null}
    </Surface>
  );
});

/** One agent conversation row, dispatched by role. No role uses boolean-soup layout. */
export const ChatMessage = memo(function ChatMessage({ block }: { block: TerminalAgentBlock }) {
  const role = agentRole(block.kind);
  switch (role) {
    case "user": return <UserMessage block={block} />;
    case "request": return <ReadOnlyRequest block={block} />;
    case "thought":
    case "tool":
    case "group": return <CollapsibleMessage block={block} />;
    case "system":
    case "status":
    case "result": return <QuietNote block={block} />;
    case "agent":
    case "unknown":
    default: return <AssistantMessage block={block} />;
  }
});

const styles = StyleSheet.create({
  plain: { minWidth: 0, gap: spacing.xs },
  row: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
  label: { flexShrink: 1, fontWeight: "600" },
  thought: { fontStyle: "italic" },
});
