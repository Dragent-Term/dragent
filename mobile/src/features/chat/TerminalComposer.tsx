/**
 * The terminal's input, as the chat's message box: what you type is pasted
 * into the terminal and Enter pressed; the terminal's own keys (Esc, Tab,
 * arrows, Ctrl-C…) sit just above it; attach, `@file`, the model and the
 * CLI | Chat switch are in the box's control row, as in the chat.
 */
import { useRef, useState, type ReactNode } from "react";
import { Keyboard, ScrollView, StyleSheet, Text } from "react-native";
import { Control, Icon } from "../../components";
import { useTheme } from "../../theme";
import { TERMINAL_KEYSTROKES } from "../shared/sessionInput";
import { COMPANION_TERMINAL_MESSAGES } from "../../terminal/companion/types";
import { draftAfterDelivery, sendDraftOnce, type DeliveryResult } from "../../terminal/companion/submitDraft";
import { ChatComposer } from "./ChatParts";
import { ChatAttachmentStrip, ChatExtrasBar, ChatExtrasTray, useChatExtras } from "./ChatExtras";

export interface TerminalInputApi {
  connected: boolean;
  sendInput: (text: string) => Promise<DeliveryResult>;
  submitDraft: (draft: string) => Promise<DeliveryResult>;
  blurTerminalInput: () => void;
}

export function TerminalComposer({
  api,
  tabId,
  workspaceRoot,
  viewSwitch,
}: {
  api: TerminalInputApi;
  tabId: number;
  workspaceRoot: string | null;
  /** The terminal / chat switch, for an agent that can be shown as a chat. */
  viewSwitch?: ReactNode;
}) {
  const { connected, sendInput, submitDraft, blurTerminalInput } = api;
  const { theme } = useTheme();
  const extras = useChatExtras({ tabId, workspaceRoot });
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);

  const send = () => {
    if (!connected || extras.uploading) return;
    const typed = draft;
    const sent = extras.message(draft);
    if (!sent.trim()) return;
    void sendDraftOnce(busy, () => submitDraft(sent), {
      onStart: () => { setPending(true); setError(null); },
      onDone: (result) => {
        setPending(false);
        if (result.ok) {
          setDraft((current) => draftAfterDelivery(current, typed));
          extras.clear();
        } else {
          setError(result.message);
        }
      },
    });
  };

  // Keys stay live during a send: the engine's queue puts them after its Enter.
  const sendKey = async (bytes: string) => {
    if (!connected) return;
    setError(null);
    try {
      const result = await sendInput(bytes);
      if (!result.ok) setError(result.message);
    } catch {
      setError(COMPANION_TERMINAL_MESSAGES.sendUnconfirmed);
    }
  };
  // The keyboard may belong to this box or to the terminal's own input.
  const hideKeyboard = () => { Keyboard.dismiss(); blurTerminalInput(); };

  const keys = (
    <ScrollView
      horizontal
      keyboardShouldPersistTaps="always"
      showsHorizontalScrollIndicator={false}
      contentContainerStyle={styles.keys}
      accessibilityLabel="Terminal keys"
    >
      {TERMINAL_KEYSTROKES.map((key) => (
        <Control
          key={key.id}
          accessibilityRole="button"
          accessibilityLabel={key.hint}
          disabled={!connected}
          onPress={() => { void sendKey(key.bytes); }}
          hitSlop={{ top: 6, bottom: 6 }}
          style={({ pressed }) => [
            styles.key,
            { backgroundColor: pressed ? theme.colors.selection : theme.colors.surfaceRaised, borderColor: theme.colors.border },
            !connected ? styles.disabled : null,
          ]}
        >
          <Text style={[styles.keyText, { color: theme.colors.text }]}>{key.label}</Text>
        </Control>
      ))}
      <Control
        accessibilityRole="button"
        accessibilityLabel="Hide keyboard"
        onPress={hideKeyboard}
        hitSlop={{ top: 6, bottom: 6 }}
        style={({ pressed }) => [
          styles.key,
          { backgroundColor: pressed ? theme.colors.selection : theme.colors.surfaceRaised, borderColor: theme.colors.border },
        ]}
      >
        <Icon name="keyboard" size={16} tone="textMuted" />
      </Control>
    </ScrollView>
  );

  return (
    <ChatComposer
      floating={false}
      value={draft}
      onChangeText={setDraft}
      placeholder="Message or command"
      accessibilityLabel="Terminal message or command"
      accessibilityHint="Send pastes it into the terminal and presses Enter. The keys above send at once."
      onSend={send}
      sending={pending}
      canSend={connected && !extras.uploading}
      error={error ?? extras.notice}
      hasContent={extras.hasAttachments}
      inputProps={{ autoCapitalize: "none", autoCorrect: false, spellCheck: false }}
      top={
        <>
          <ChatExtrasTray extras={extras} draft={draft} onDraft={setDraft} />
          {keys}
        </>
      }
      inside={<ChatAttachmentStrip extras={extras} />}
      bottom={<ChatExtrasBar extras={extras}>{viewSwitch}</ChatExtrasBar>}
    />
  );
}

const styles = StyleSheet.create({
  keys: { flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: 2 },
  key: {
    minWidth: 40,
    height: 32,
    paddingHorizontal: 10,
    borderRadius: 9,
    borderWidth: StyleSheet.hairlineWidth,
    alignItems: "center",
    justifyContent: "center",
  },
  keyText: { fontSize: 13, fontWeight: "500" },
  disabled: { opacity: 0.4 },
});
