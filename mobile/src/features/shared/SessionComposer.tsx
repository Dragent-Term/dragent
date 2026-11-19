import { useEffect, useMemo, useReducer, useState } from "react";
import { ScrollView, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { AppText, Button, FormField, Icon, IconButton, SegmentedControl, StatusBadge } from "../../components";
import { FieldInput } from "../../components/FieldInput";
import { useTheme } from "../../theme";
import { useAsyncAction } from "../../hooks/useAsyncAction";
import { spacing } from "../../theme/tokens";
import { Feedback } from "./Feedback";
import {
  TERMINAL_KEYSTROKES,
  canRecallNewer,
  canRecallOlder,
  canSendToSession,
  commandToSend,
  draftReducer,
  initialDrafts,
  isUsableCommand,
  isUsableKeystrokeText,
  keystrokesToSend,
  prepareAgentDraft,
  pushCommandHistory,
  sendBlockerReason,
  stepCommandHistory,
  tooLargeMessage,
  type SessionWriters,
} from "./sessionInput";

export type SessionViewMode = "terminal" | "agent";
export type TerminalEntryMode = "command" | "input";

export interface SessionComposerProps {
  /** Saved-session id; drafts reset whenever it changes so text never leaks rooms. */
  sessionId: string;
  mode: SessionViewMode;
  connected: boolean;
  role: "viewer" | "controller" | null;
  /** Latest host agent status, if the host shared one. */
  aiStatus: string | null;
  /** Host reports a command currently running. */
  commandRunning: boolean;
  writers: SessionWriters | null;
  environment?: "live" | "preview";
  layout?: "form" | "dock";
  /** Offer only input operations supported by the attached transport. */
  terminalEntryModes?: readonly TerminalEntryMode[];
}

const TERMINAL_ENTRY_OPTIONS = [
  { id: "command", label: "Command", hint: "Run a full command" },
  { id: "input", label: "Input", hint: "Send keystrokes to the running terminal" },
] as const;

/**
 * Mode-specific composer for a live session. The screen keeps this mounted
 * for the whole session: when the role is not controller it renders nothing
 * but keeps drafts, so a transient disconnect or downgrade never discards
 * text. Every write is gated by a live controller connection; drafts survive
 * failures and clear on success or explicit cancel only.
 */
export function SessionComposer({
  sessionId, mode, connected, role, aiStatus, commandRunning, writers, environment = "live", layout = "form", terminalEntryModes,
}: SessionComposerProps) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const entryOptions = terminalEntryModes ? TERMINAL_ENTRY_OPTIONS.filter((option) => terminalEntryModes.includes(option.id)) : TERMINAL_ENTRY_OPTIONS;
  const firstEntry = entryOptions[0]?.id ?? "input";
  const writable = writers !== null && (mode === "agent" || entryOptions.length > 0) && canSendToSession({ status: connected ? "connected" : "idle", role });
  const [drafts, dispatchDrafts] = useReducer(draftReducer, sessionId, initialDrafts);
  const [selectedEntry, setEntry] = useState<TerminalEntryMode>(firstEntry);
  const entry = entryOptions.some((option) => option.id === selectedEntry) ? selectedEntry : firstEntry;
  const [keyError, setKeyError] = useState<string | null>(null);
  const [keysVisible, setKeysVisible] = useState(false);
  // Per-session command history (oldest → newest). Reset on session switch
  // so commands never leak rooms; survives disconnects and role flaps.
  const [commandHistory, setCommandHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState<number | null>(null);
  const [stashedCommand, setStashedCommand] = useState("");
  const question = useMemo(() => prepareAgentDraft(drafts.query), [drafts.query]);

  useEffect(() => {
    dispatchDrafts({ type: "session", sessionId });
    setEntry(firstEntry);
    setKeyError(null);
    setKeysVisible(false);
    setCommandHistory([]);
    setHistoryIndex(null);
    setStashedCommand("");
  }, [sessionId, firstEntry]);

  const exitHistoryBrowse = () => {
    setHistoryIndex(null);
  };

  const editCommand = (value: string) => {
    // Manual edits supersede browsing: the new text is the draft.
    if (historyIndex !== null) setHistoryIndex(null);
    dispatchDrafts({ type: "edit", field: "command", value });
  };

  const recallOlder = () => {
    if (commandHistory.length === 0) return;
    const draftForRecall = historyIndex === null ? drafts.command : stashedCommand;
    if (historyIndex === null) setStashedCommand(drafts.command);
    const step = stepCommandHistory(commandHistory, historyIndex, "older", draftForRecall);
    setHistoryIndex(step.index);
    dispatchDrafts({ type: "edit", field: "command", value: step.value });
  };

  const recallNewer = () => {
    if (historyIndex === null) return;
    const step = stepCommandHistory(commandHistory, historyIndex, "newer", stashedCommand);
    setHistoryIndex(step.index);
    dispatchDrafts({ type: "edit", field: "command", value: step.value });
  };

  const clearCommand = () => {
    exitHistoryBrowse();
    dispatchDrafts({ type: "cancel", field: "command" });
  };

  const clearKeystrokes = () => {
    dispatchDrafts({ type: "cancel", field: "keystrokes" });
  };

  const clearQuery = () => {
    dispatchDrafts({ type: "cancel", field: "query" });
  };

  const runCommand = useAsyncAction(async () => {
    if (!isUsableCommand(drafts.command)) throw new Error("Type a command to run first.");
    const payload = commandToSend(drafts.command);
    if (payload === null) throw new Error(tooLargeMessage());
    if (!writers?.executeCommand(payload)) {
      throw new Error("This command was not sent. Connect with Edit access and try again.");
    }
    // History only records text the transport accepted; browsing resets.
    setCommandHistory((previous) => pushCommandHistory(previous, payload));
    setHistoryIndex(null);
    setStashedCommand("");
    dispatchDrafts({ type: "sent", field: "command" });
  });

  const sendKeys = useAsyncAction(async () => {
    if (!isUsableKeystrokeText(drafts.keystrokes)) throw new Error("Type something to send first.");
    const payload = keystrokesToSend(drafts.keystrokes);
    if (payload === null) throw new Error(tooLargeMessage());
    if (!writers?.sendInput(payload)) {
      throw new Error("These keystrokes were not sent. Connect with Edit access and try again.");
    }
    dispatchDrafts({ type: "sent", field: "keystrokes" });
  });

  const askAgent = useAsyncAction(async () => {
    if (!question.ok) throw new Error(question.message);
    if (!writers?.sendAgentQuery(question.text)) {
      throw new Error("This question was not sent. Connect with Edit access and try again.");
    }
    dispatchDrafts({ type: "sent", field: "query" });
  });

  const stopAgentAction = useAsyncAction(async () => {
    if (!writers?.stopAgent()) {
      throw new Error("The stop request was not sent. Connect with Edit access and try again.");
    }
  });

  const sendKey = (bytes: string, label: string) => {
    if (!writers) {
      setKeyError("The session is not ready yet. Try again in a moment.");
      return;
    }
    const blocker = !writable ? sendBlockerReason({ status: connected ? "connected" : "idle", role }) : null;
    if (blocker) {
      setKeyError(blocker);
      return;
    }
    const sent = writers.sendInput(bytes);
    setKeyError(sent ? null : `That key (${label}) was not sent. Connect with Edit access and try again.`);
  };

  if (role !== "controller") return null;

  const olderDisabled = !canRecallOlder(commandHistory, historyIndex);
  const newerDisabled = !canRecallNewer(commandHistory, historyIndex);
  const historyPosition = commandHistory.length === 0
    ? "No history yet"
    : historyIndex === null
      ? `${commandHistory.length} ${commandHistory.length === 1 ? "command" : "commands"}`
      : `${historyIndex + 1} of ${commandHistory.length}`;

  const historyAccessory = (variant: "dock" | "form") => (
    <View
      accessibilityRole="toolbar"
      accessibilityLabel="Command history"
      style={variant === "dock" ? styles.historyBarDock : styles.historyBar}
    >
      <ScrollView
        horizontal
        keyboardShouldPersistTaps="handled"
        showsHorizontalScrollIndicator={false}
        style={styles.historyScroll}
        contentContainerStyle={styles.historyContent}
      >
        <Button
          label="↑"
          accessibilityLabel="Recall older command"
          accessibilityHint="Replaces the command draft with the previous sent command"
          size="sm"
          variant="ghost"
          disabled={olderDisabled}
          onPress={recallOlder}
          style={styles.historyButton}
        />
        <Button
          label="↓"
          accessibilityLabel="Recall newer command"
          accessibilityHint="Moves forward through sent commands, restoring your draft at the end"
          size="sm"
          variant="ghost"
          disabled={newerDisabled}
          onPress={recallNewer}
          style={styles.historyButton}
        />
        <Button
          label="Clear"
          accessibilityLabel="Clear command draft"
          accessibilityHint="Discards the command draft without sending"
          size="sm"
          variant="ghost"
          disabled={drafts.command.length === 0}
          onPress={clearCommand}
          style={styles.historyButton}
        />
        <AppText variant="caption" tone="textMuted" style={styles.historyPosition}>
          {historyPosition}
        </AppText>
      </ScrollView>
    </View>
  );

  const keysAccessory = (
    <ScrollView
      horizontal
      keyboardShouldPersistTaps="handled"
      accessibilityRole="toolbar"
      accessibilityLabel="Terminal keys"
      style={styles.keysScroll}
      contentContainerStyle={styles.keysContent}
    >
      {TERMINAL_KEYSTROKES.map((key) => (
        <Button
          key={key.id}
          label={key.label}
          size="sm"
          variant="ghost"
          accessibilityLabel={key.hint}
          disabled={!writable}
          onPress={() => sendKey(key.bytes, key.label)}
          style={styles.keyButton}
        />
      ))}
    </ScrollView>
  );

  if (layout === "dock") {
    const agent = mode === "agent";
    const input = !agent && entry === "input";
    const field = agent ? "query" : input ? "keystrokes" : "command";
    const action = agent ? askAgent : input ? sendKeys : runCommand;
    const valid = agent ? question.ok : input ? isUsableKeystrokeText(drafts.keystrokes) : isUsableCommand(drafts.command);
    return (
      <View style={[styles.dock, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border, paddingBottom: 6 + insets.bottom }]}>
        {!agent && entry === "command" ? historyAccessory("dock") : null}
        {!agent && keysVisible ? (
          <View style={styles.section}>
            {entryOptions.length > 1 ? <SegmentedControl label="Terminal input mode" layout="scroll" options={entryOptions} value={entry} onChange={(next) => { setEntry(next); setHistoryIndex(null); }} /> : null}
            {keysAccessory}
          </View>
        ) : null}
        <View style={styles.dockRow}>
          {!agent ? <IconButton name="keyboard" label={keysVisible ? "Hide terminal keys" : "Show terminal keys"}
            accessibilityState={{ expanded: keysVisible }} onPress={() => setKeysVisible((visible) => !visible)} /> : null}
          <FieldInput containerStyle={styles.dockField} style={styles.dockInput}
            leading={agent ? undefined : <Icon name="terminal" size={14} />}
            accessibilityLabel={agent ? "Ask the agent" : input ? "Send keystrokes" : "Run a command"}
            accessibilityHint={environment === "preview" ? "Local preview. Nothing is sent to a desktop." : undefined}
            value={drafts[field]}
            onChangeText={(value) => {
              if (field === "command") editCommand(value);
              else dispatchDrafts({ type: "edit", field, value });
            }}
            placeholder={agent ? "Message the agent…" : input ? "Send terminal input…" : "Enter a command…"}
            multiline={agent || input}
            autoCapitalize="none"
            autoCorrect={false}
            editable={connected}
            returnKeyType={agent || input ? "default" : "send"}
            onSubmitEditing={() => { if (!agent && !input && valid) void action.run(); }} />
          <Button label={agent ? "Ask" : input ? "Send" : "Run"} size="sm" loading={action.pending}
            disabled={!writable || !valid} onPress={() => { void action.run(); }} />
          {agent ? <IconButton name="stop" label="Stop agent" disabled={!writable || stopAgentAction.pending}
            onPress={() => { void stopAgentAction.run(); }} /> : null}
          {!agent && input ? (
            <Button
              label="Clear"
              accessibilityLabel="Clear terminal input draft"
              size="sm"
              variant="ghost"
              disabled={drafts.keystrokes.length === 0}
              onPress={clearKeystrokes}
            />
          ) : null}
        </View>
        <Feedback message={action.error ?? (agent ? stopAgentAction.error : keyError)
          ?? (agent && !question.ok && question.error === "too-large" ? question.message : null)} />
      </View>
    );
  }

  if (mode === "agent") {
    return (
      <View style={styles.section}>
        {aiStatus ? <StatusBadge label={aiStatus} tone="neutral" /> : null}
        <FormField
          label="Ask the agent"
          value={drafts.query}
          onChangeText={(value) => dispatchDrafts({ type: "edit", field: "query", value })}
          multiline
          autoCapitalize="sentences"
          autoCorrect
          editable={connected}
          placeholder="Ask about this session…"
          hint={environment === "preview" ? "Try a question. Responses are local preview examples." : "The host answers in the Agent view. Nothing is sent until you ask."}
        />
        <View style={styles.actions}>
          <Button
            label="Ask"
            loading={askAgent.pending}
            disabled={!writable || !question.ok}
            onPress={() => { void askAgent.run(); }}
          />
          <Button
            label="Stop agent"
            variant="ghost"
            loading={stopAgentAction.pending}
            disabled={!writable}
            accessibilityHint={environment === "preview" ? "Pauses the local preview agent." : "Asks the host to stop the agent. The host confirms by sharing new output."}
            onPress={() => { void stopAgentAction.run(); }}
          />
          <Button
            label="Clear"
            variant="ghost"
            disabled={drafts.query.length === 0}
            accessibilityLabel="Clear agent draft"
            onPress={clearQuery}
          />
        </View>
        <Feedback message={askAgent.error ?? stopAgentAction.error ?? (!question.ok && question.error === "too-large" ? question.message : null)} />
      </View>
    );
  }

  return (
    <View style={styles.section}>
      {commandRunning ? (
        <AppText variant="caption" tone="textMuted">A command is running on the host.</AppText>
      ) : null}
      {entryOptions.length > 1 ? <SegmentedControl
        label="Terminal input mode"
        options={entryOptions}
        value={entry}
        onChange={(next) => { setEntry(next); setHistoryIndex(null); }}
      /> : null}
      {entry === "command" ? (
        <View>
          {historyAccessory("form")}
          <View style={styles.row}>
            <View style={styles.field}>
              <FormField
                label="Run a command"
                value={drafts.command}
                onChangeText={editCommand}
                autoCapitalize="none"
                autoCorrect={false}
                editable={connected}
                returnKeyType="send"
                placeholder="make test"
                hint={environment === "preview" ? "Try a command. Output is simulated in this preview." : "Runs on the host exactly as written."}
                onSubmitEditing={() => { if (isUsableCommand(drafts.command)) void runCommand.run(); }}
              />
            </View>
            <View style={styles.sendButton}>
              <Button
                label="Run"
                loading={runCommand.pending}
                disabled={!writable || !isUsableCommand(drafts.command)}
                onPress={() => { void runCommand.run(); }}
              />
            </View>
          </View>
          <Feedback message={runCommand.error} />
        </View>
      ) : (
        <View>
          <View style={styles.row}>
            <View style={styles.field}>
              <FormField
                label="Send keystrokes"
                value={drafts.keystrokes}
                onChangeText={(value) => dispatchDrafts({ type: "edit", field: "keystrokes", value })}
                multiline
                autoCapitalize="none"
                autoCorrect={false}
                editable={connected}
                placeholder="Type to the running terminal…"
                hint="Sends text exactly, including line breaks."
                style={styles.keysInput}
              />
            </View>
            <View style={styles.sendButton}>
              <Button
                label="Send"
                loading={sendKeys.pending}
                disabled={!writable || !isUsableKeystrokeText(drafts.keystrokes)}
                onPress={() => { void sendKeys.run(); }}
              />
            </View>
            <View style={styles.sendButton}>
              <Button
                label="Clear"
                variant="ghost"
                disabled={drafts.keystrokes.length === 0}
                accessibilityLabel="Clear terminal input draft"
                onPress={clearKeystrokes}
              />
            </View>
          </View>
          <Feedback message={sendKeys.error} />
          {keysAccessory}
          <Feedback message={keyError} />
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  dock: { borderTopWidth: 1, padding: 6, gap: 4 },
  dockRow: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 4 },
  dockField: { flex: 1, flexGrow: 1, flexShrink: 1, flexBasis: 100, minWidth: 0, borderRadius: 6, paddingHorizontal: 8 },
  dockInput: { minHeight: 46, maxHeight: 100 },
  section: { gap: spacing.sm },
  row: { flexDirection: "row", flexWrap: "wrap", alignItems: "flex-end", gap: spacing.sm },
  field: { flexGrow: 1, flexShrink: 1, flexBasis: 200, minWidth: 0 },
  sendButton: { flexShrink: 0, paddingBottom: 1 },
  keysInput: { minHeight: 64 },
  keysScroll: { maxHeight: 60, flexGrow: 0, flexShrink: 1, minWidth: 0 },
  keysContent: { alignItems: "center", gap: spacing.xs, paddingRight: spacing.sm },
  keyButton: { minHeight: 48, minWidth: 48 },
  historyBar: { gap: spacing.xs },
  historyBarDock: { gap: spacing.xs, minWidth: 0 },
  historyScroll: { flexGrow: 0, flexShrink: 1, minWidth: 0 },
  historyContent: { alignItems: "center", gap: spacing.xs, paddingRight: spacing.sm },
  historyButton: { minHeight: 48, minWidth: 48 },
  historyPosition: { flexShrink: 1, minWidth: 0 },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
});
