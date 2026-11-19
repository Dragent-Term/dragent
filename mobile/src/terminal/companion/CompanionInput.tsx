import { useRef, useState } from "react";
import { Keyboard, ScrollView, StyleSheet, View } from "react-native";
import { Button } from "../../components";
import { FieldInput } from "../../components/FieldInput";
import { Feedback } from "../../features/shared/Feedback";
import { TERMINAL_KEYSTROKES } from "../../features/shared/sessionInput";
import { useTheme } from "../../theme";
import { COMPANION_TERMINAL_MESSAGES } from "./types";
import { draftAfterDelivery, sendDraftOnce, type DeliveryResult } from "./submitDraft";

/** Draft submission and literal terminal keys are distinct operations. */
export function CompanionInput({ connected, sendInput, submitDraft, blurTerminalInput }: {
  connected: boolean;
  sendInput: (text: string) => Promise<DeliveryResult>;
  submitDraft: (draft: string) => Promise<DeliveryResult>;
  blurTerminalInput: () => void;
}) {
  const { theme } = useTheme();
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);
  const sendDraft = () => {
    if (!connected) return;
    const sent = draft;
    void sendDraftOnce(busy, () => submitDraft(sent), {
      onStart: () => { setPending(true); setError(null); },
      onDone: (result) => {
        setPending(false);
        if (result.ok) setDraft((current) => draftAfterDelivery(current, sent));
        else setError(result.message);
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
    } catch { setError(COMPANION_TERMINAL_MESSAGES.sendUnconfirmed); }
  };
  // The keyboard may belong to this field or to the terminal's own input, which
  // Keyboard.dismiss() cannot reach, so both are asked.
  const hideKeyboard = () => { Keyboard.dismiss(); blurTerminalInput(); };
  return <View style={[styles.dock, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
    <ScrollView horizontal keyboardShouldPersistTaps="always" showsHorizontalScrollIndicator={false}
      contentContainerStyle={styles.keys} accessibilityLabel="Terminal keys">
      {TERMINAL_KEYSTROKES.map((key) => <Button key={key.id} label={key.label} accessibilityLabel={key.hint}
        size="sm" variant="ghost" style={styles.tapTarget} disabled={!connected} onPress={() => { void sendKey(key.bytes); }} />)}
      <Button label="Hide keyboard" size="sm" variant="ghost" style={styles.tapTarget} onPress={hideKeyboard} />
    </ScrollView>
    <View style={styles.row}>
      {/* Always editable: on iOS a non-editable field drops the keyboard, and a send or a connection blip must not. */}
      <FieldInput containerStyle={styles.field} style={styles.input} value={draft} onChangeText={setDraft}
        placeholder="Message or command…" accessibilityLabel="Terminal message or command"
        accessibilityHint="Send submits the text followed by Enter. Terminal keys send immediately."
        multiline autoCapitalize="none" autoCorrect={false} />
      <Button label="Send ↵" accessibilityLabel="Send text and press Enter" size="sm" style={styles.tapTarget} loading={pending}
        disabled={!connected || !draft.trim()} onPress={sendDraft} />
    </View>
    <Feedback message={error} />
  </View>;
}
const styles = StyleSheet.create({
  dock: { borderTopWidth: 1, padding: 6, gap: 4 },
  keys: { flexDirection: "row", alignItems: "center" },
  row: { flexDirection: "row", alignItems: "flex-end", gap: 8 },
  field: { flex: 1, minWidth: 0 },
  input: { minHeight: 44, maxHeight: 120 },
  tapTarget: { minHeight: 44 },
});
