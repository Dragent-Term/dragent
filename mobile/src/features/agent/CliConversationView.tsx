import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ActivityIndicator, AppState, FlatList, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useFocusEffect } from "expo-router";
import { AgentMark, AppText, Button } from "../../components";
import { useTheme } from "../../theme";
import { useMachine } from "../../machines/MachineContext";
import { useSoftKeyboard } from "../../terminal/companion/useSoftKeyboard";
import type { RelayTab } from "../../transport/relayCatalog";
import { readAgentTranscript, sendTabText, type TranscriptRow } from "../../transport/relayRpcMethods";
import { useStickToBottom } from "../chat/useStickToBottom";
import { layoutTranscript, stepFromToolRow, type Entry, type Line } from "../chat/transcript";
import { TranscriptLine, type LineHandlers } from "../chat/TranscriptParts";
import { ChatComposer, ComposerButton, FloatingPill } from "../chat/ChatParts";
import { ChatAttachmentStrip, ChatExtrasBar, ChatExtrasTray, useChatExtras } from "../chat/ChatExtras";
import { useVoiceConversation } from "../chat/useVoiceConversation";
import { VoicePanel } from "../chat/VoicePanel";

/** How often the conversation is re-read: quicker while the agent works. */
const POLL_WORKING_MS = 1500;
const POLL_IDLE_MS = 5000;
/** A sent message nobody echoed back after this long stops waiting. */
const OUTBOX_GIVE_UP_MS = 60_000;

const NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  opencode: "OpenCode",
  mimo: "MiMo Code",
  gemini: "Gemini",
  qwen: "Qwen Code",
  copilot: "Copilot",
  "cursor-agent": "Cursor",
};

/** A message sent from here that the transcript hasn't shown yet. */
interface Outgoing {
  readonly id: string;
  readonly text: string;
  /** How many of your messages with this text the transcript had when it was sent. */
  readonly seen: number;
  readonly at: number;
  readonly failed: string | null;
}

type CliLine = Line<never>;

function sameText(row: TranscriptRow, text: string): boolean {
  return row.role === "user" && row.text.trim() === text;
}

/** The transcript (and what's still on its way) as conversation entries. */
function entriesOf(rows: readonly TranscriptRow[], outbox: readonly Outgoing[], live: boolean): Entry<never>[] {
  const entries: Entry<never>[] = rows.map((row): Entry<never> => {
    if (row.role === "user") return { kind: "you", id: row.id, text: row.text, at: row.at };
    if (row.role === "tool") return { kind: "step", step: stepFromToolRow(row, live), at: row.at };
    return { kind: "reply", id: row.id, text: row.text, at: row.at, streaming: false };
  });
  for (const sent of outbox) {
    entries.push({
      kind: "you",
      id: `out:${sent.id}`,
      text: sent.text,
      at: sent.at,
      note: sent.failed ? { kind: "failed", reason: sent.failed } : { kind: "sending" },
    });
  }
  return entries;
}

/**
 * A CLI agent (Claude Code, Codex) as a conversation, read from the
 * transcript it writes on the desktop and laid out as the desktop chat lays
 * it out: your message, the turn's clock, its replies, its tool calls on a
 * rail. The message box types into its terminal. When it waits on a choice
 * only its terminal can take, a pill says so and opens the terminal.
 * `onUnsupported` hands over to the terminal for an agent the desktop can't read.
 */
export function CliConversationView({ tab, onOpenTerminal, onUnsupported, workspaceRoot = null, viewSwitch }: {
  tab: RelayTab;
  onOpenTerminal: () => void;
  onUnsupported: () => void;
  /** The tab's workspace on the desktop, where `@` finds files. */
  workspaceRoot?: string | null;
  /** The terminal / chat switch; without it, a button opens the terminal. */
  viewSwitch?: ReactNode;
}) {
  const extras = useChatExtras({ tabId: tab.id, workspaceRoot });
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const keyboard = useSoftKeyboard(insets.bottom);
  const { rpc } = useMachine();
  const agent = (tab.agent_kind ?? "").toLowerCase();
  const [rows, setRows] = useState<readonly TranscriptRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [outbox, setOutbox] = useState<readonly Outgoing[]>([]);
  const [dockHeight, setDockHeight] = useState(60);
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
  const stick = useStickToBottom<CliLine>();
  const working = tab.agent_state === "working";
  const waiting = tab.attention === "needs_input";
  const name = NAMES[agent] ?? (tab.agent_kind || "The agent");

  // A sent message stops being "Sending…" once the transcript shows it.
  useEffect(() => {
    if (!rows) return;
    setOutbox((current) => {
      const now = Date.now();
      const next = current.filter((sent) => {
        if (sent.failed) return true;
        if (now - sent.at > OUTBOX_GIVE_UP_MS) return false;
        return rows.filter((row) => sameText(row, sent.text)).length <= sent.seen;
      });
      return next.length === current.length ? current : next;
    });
  }, [rows]);

  const pending = outbox.some((sent) => !sent.failed);
  const lines = useMemo(
    () => layoutTranscript(entriesOf(rows ?? [], outbox, working), { working: working || pending, waiting }),
    [rows, outbox, working, pending, waiting],
  );

  const lastRead = useRef<string | null>(null);
  const read = useCallback(async () => {
    if (!rpc) return;
    try {
      const next = await readAgentTranscript(rpc, tab.id);
      if (!next.supported) {
        onUnsupported();
        return;
      }
      // Most reads find nothing new: keep the same rows, so nothing redraws.
      const seen = JSON.stringify(next.rows);
      if (seen !== lastRead.current) {
        lastRead.current = seen;
        setRows(next.rows);
      }
      setError(null);
    } catch (failure) {
      const text = (failure as { message?: unknown } | null)?.message;
      setError(typeof text === "string" && text ? text : "The conversation isn't available right now.");
    }
  }, [rpc, tab.id, onUnsupported]);

  // Read now, then on a beat while this screen is in front.
  const busy = working || pending;
  useFocusEffect(useCallback(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      if (!active) return;
      if (AppState.currentState === "active") await read();
      if (active) timer = setTimeout(() => void tick(), busy ? POLL_WORKING_MS : POLL_IDLE_MS);
    };
    void tick();
    return () => {
      active = false;
      if (timer) clearTimeout(timer);
    };
  }, [read, busy]));

  // Back from the background: read now rather than at the next beat.
  useEffect(() => {
    const sub = AppState.addEventListener("change", (state) => {
      if (state === "active") void read();
    });
    return () => sub.remove();
  }, [read]);

  // The keyboard and the message box take room from the list: keep the newest line in view.
  const { pin } = stick;
  useEffect(() => {
    pin();
  }, [keyboard.visible, dockHeight, pin]);

  const deliver = useCallback(async (sent: Outgoing): Promise<boolean> => {
    if (!rpc) return false;
    try {
      await sendTabText(rpc, tab.id, sent.text);
      setTimeout(() => void read(), 500);
      return true;
    } catch (failure) {
      const message = (failure as { message?: unknown } | null)?.message;
      const reason = typeof message === "string" && message ? message : "The desktop couldn't deliver that.";
      setOutbox((current) => current.map((entry) => (entry.id === sent.id ? { ...entry, failed: reason } : entry)));
      return false;
    }
  }, [rpc, tab.id, read]);

  // Voice mode: what you said goes like a typed message; the agent's last
  // finished reply is read aloud.
  const sendSpoken = useCallback(async (text: string) => {
    if (!rpc) return false;
    const sent: Outgoing = {
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      text,
      seen: (rows ?? []).filter((row) => sameText(row, text)).length,
      at: Date.now(),
      failed: null,
    };
    setOutbox((current) => [...current, sent]);
    stick.follow();
    return deliver(sent);
  }, [rpc, rows, stick, deliver]);
  const lastReply = useMemo(() => {
    const all = rows ?? [];
    let mine = -1;
    for (let index = all.length - 1; index >= 0; index -= 1) if (all[index]!.role === "user") { mine = index; break; }
    for (let index = all.length - 1; index > mine; index -= 1) {
      const row = all[index]!;
      if (row.role === "assistant" && row.text.trim()) return { id: row.id, text: row.text };
    }
    return null;
  }, [rows]);
  const voice = useVoiceConversation({ send: sendSpoken, reply: lastReply, busy: working || pending });

  const send = () => {
    const text = extras.message(draft).trim();
    if (!rpc || text === "" || extras.uploading) return;
    const sent: Outgoing = {
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      text,
      seen: (rows ?? []).filter((row) => sameText(row, text)).length,
      at: Date.now(),
      failed: null,
    };
    setOutbox((current) => [...current, sent]);
    setDraft("");
    extras.clear();
    stick.follow();
    void deliver(sent);
  };

  const handlers = useMemo<LineHandlers<never>>(() => ({
    isOpen: (key) => opened.has(key),
    onToggle: (key) => setOpened((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    }),
    renderCard: () => null,
    onRetry: (id) => {
      const sent = outbox.find((entry) => `out:${entry.id}` === id);
      if (!sent) return;
      const again = { ...sent, failed: null, at: Date.now() };
      setOutbox((current) => current.map((entry) => (entry.id === sent.id ? again : entry)));
      void deliver(again);
    },
    onDiscard: (id) => setOutbox((current) => current.filter((entry) => `out:${entry.id}` !== id)),
  }), [opened, outbox, deliver]);

  return (
    <View style={[styles.frame, { paddingBottom: keyboard.height }]}>
      {/* Everything floats inside this body, which ends at the top of the keyboard. */}
      <View style={styles.flex}>
      {rows === null ? (
        <View style={[styles.center, { paddingBottom: dockHeight }]}>
          {error ? (
            <>
              <AppText variant="body" style={styles.centerText}>{error}</AppText>
              <Button label="Open the terminal" size="sm" variant="ghost" onPress={onOpenTerminal} />
            </>
          ) : (
            <ActivityIndicator color={theme.colors.textMuted} />
          )}
        </View>
      ) : (
        <FlatList
          {...stick.listProps}
          data={lines}
          keyExtractor={(line) => line.key}
          renderItem={({ item }) => <TranscriptLine line={item} handlers={handlers} />}
          extraData={handlers}
          style={styles.flex}
          contentContainerStyle={lines.length === 0 ? styles.emptyContent : styles.content}
          keyboardShouldPersistTaps="handled"
          keyboardDismissMode="interactive"
          accessibilityLabel={`${name} conversation`}
          ListEmptyComponent={
            <View style={styles.center}>
              <AgentMark agent={agent} size={40} />
              <AppText style={styles.emptyTitle}>What should {name} do?</AppText>
              <AppText variant="caption" tone="textMuted" style={styles.centerText}>It works in this workspace on your desktop.</AppText>
            </View>
          }
          ListFooterComponent={<View style={{ height: dockHeight + 16 }} />}
        />
      )}
      {waiting || stick.away ? (
        <View style={[styles.floating, { bottom: dockHeight + 10 }]} pointerEvents="box-none">
          {waiting ? (
            <FloatingPill label={`${name} needs your answer`} icon="message" tone="attention" onPress={onOpenTerminal} />
          ) : (
            <FloatingPill label="Latest" icon="chevron-down" onPress={stick.jumpToEnd} />
          )}
        </View>
      ) : null}
      {voice.active ? (
        <VoicePanel
          phase={voice.phase}
          heard={voice.heard}
          level={voice.level}
          error={voice.error}
          name={name}
          onNext={voice.next}
          onEnd={voice.end}
          bottom={0}
        />
      ) : (
      <ChatComposer
        value={draft}
        onChangeText={setDraft}
        placeholder={working ? `Add to what ${name} is doing` : `Message ${name}`}
        accessibilityLabel={`Message to ${name}`}
        onSend={send}
        sending={false}
        canSend={rpc !== null && !extras.uploading}
        onHeight={setDockHeight}
        top={<ChatExtrasTray extras={extras} draft={draft} onDraft={setDraft} />}
        inside={<ChatAttachmentStrip extras={extras} />}
        bottom={
          <ChatExtrasBar extras={extras}>
            {viewSwitch ?? <ComposerButton icon="terminal" label="Open the terminal" onPress={onOpenTerminal} />}
            <ComposerButton icon="voice" label={`Talk with ${name}`} onPress={voice.start} />
          </ChatExtrasBar>
        }
        hasContent={extras.hasAttachments}
        error={extras.notice}
      />
      )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  flex: { flex: 1, minHeight: 0 },
  center: { flex: 1, alignItems: "center", justifyContent: "center", gap: 10, padding: 32 },
  centerText: { textAlign: "center" },
  emptyTitle: { fontSize: 19, lineHeight: 26, fontWeight: "600", textAlign: "center" },
  content: { paddingTop: 16 },
  emptyContent: { flexGrow: 1 },
  floating: { position: "absolute", left: 0, right: 0, alignItems: "center" },
});
