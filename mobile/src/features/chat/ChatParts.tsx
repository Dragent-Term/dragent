/**
 * The message box floating over the bottom of an agent conversation, and the
 * pills that float above it.
 */
import { useEffect, useRef, useState, type ReactNode } from "react";
import { ActivityIndicator, Platform, StyleSheet, Text, TextInput, View, type TextInputProps } from "react-native";
import { ArrowUp, Mic } from "lucide-react-native";
import { METER_BARS, appendDictation, recordingTime } from "./dictationText";
import { useDictation, type DictationPhase } from "./useDictation";
import { AppText, Control, Icon, type IconName } from "../../components";
import { isDarkBackground, useTheme } from "../../theme";
import { inputColorsFor } from "../../theme/inputColors";
const CONTROL = 38;

/** A round button inside the message box (Terminal, and the like). */
export function ComposerButton({ icon, label, onPress }: { icon: IconName; label: string; onPress: () => void }) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      hitSlop={4}
      style={({ pressed }) => [styles.control, { backgroundColor: pressed ? theme.colors.selection : theme.colors.surface }]}
    >
      <Icon name={icon} size={18} tone="textMuted" />
    </Control>
  );
}

export type TabView = "terminal" | "conversation";

/**
 * Switches the tab between its terminal and its chat, from the message box.
 * The icon shows where it goes: the terminal from the chat, the chat from
 * the terminal.
 */
export function ViewSwitch({ value, onChange }: { value: TabView; onChange: (next: TabView) => void }) {
  const next: TabView = value === "conversation" ? "terminal" : "conversation";
  return (
    <ComposerButton
      icon={next === "terminal" ? "terminal" : "message"}
      label={next === "terminal" ? "Show the terminal" : "Show the conversation"}
      onPress={() => onChange(next)}
    />
  );
}

/**
 * The message box, floating over the bottom of the conversation: one rounded
 * pill that grows with what you type. The round button on the right sends;
 * while the agent works and the box is empty it stops the agent instead. The
 * mic beside it dictates: settled words are added to the end of the message,
 * words still being recognised show above it until they settle.
 * `onHeight` reports how much of the conversation it covers.
 */
export function ChatComposer({
  value, onChangeText, placeholder, accessibilityLabel, accessibilityHint, onSend, sending, canSend,
  onStop, stopping = false, error, leading, onHeight, top, bottom, inside, hasContent = false,
  floating = true, inputProps,
}: {
  value: string;
  onChangeText: (text: string) => void;
  placeholder: string;
  accessibilityLabel: string;
  accessibilityHint?: string;
  onSend: () => void;
  sending: boolean;
  canSend: boolean;
  onStop?: (() => void) | undefined;
  stopping?: boolean;
  error?: string | null;
  leading?: ReactNode;
  onHeight?: (height: number) => void;
  /** Above the box: menus, suggestions, attached files. */
  top?: ReactNode;
  /** In the box's control row, after `leading`: + and the model. */
  bottom?: ReactNode;
  /** Inside the box, above what you type: attached files. */
  inside?: ReactNode;
  /** Something besides the text is ready to send (an attached file). */
  hasContent?: boolean;
  /** Float over the bottom of a list (the chat), or sit in the layout (the terminal). */
  floating?: boolean;
  /** Extra settings for the text field (a terminal turns off capitals and autocorrect). */
  inputProps?: Pick<TextInputProps, "autoCapitalize" | "autoCorrect" | "spellCheck">;
}) {
  const { theme } = useTheme();
  const caretColors = inputColorsFor(theme.colors.accent, theme.colors.background, Platform.OS);
  const latest = useRef(value);
  latest.current = value;
  const dictation = useDictation((heard) => onChangeText(appendDictation(latest.current, heard)));
  const dictating = dictation.phase !== "idle";
  const empty = value.trim() === "" && !hasContent;
  const stopMode = onStop !== undefined && empty && !sending;
  const ready = canSend && !empty && !sending;
  const enabled = stopMode ? !stopping : ready;
  const spinning = sending || (stopMode && stopping);
  const solid = stopMode || ready;
  // Send while there is something to send, Stop while the agent works and there isn't.
  const showAction = !dictating && (!empty || stopMode || sending);
  const shownError = error ?? dictation.error;
  return (
    <View
      style={floating ? styles.dock : styles.inlineDock}
      pointerEvents="box-none"
      onLayout={(event) => onHeight?.(event.nativeEvent.layout.height)}
    >
      {top}
      {shownError ? (
        <View style={[styles.errorPill, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
          <AppText variant="caption" tone="danger" accessibilityRole="alert">{shownError}</AppText>
        </View>
      ) : null}
      <View
        style={[
          styles.pill,
          { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border, shadowColor: "#000" },
        ]}
      >
        {inside}
        {dictation.heard ? (
          <Text numberOfLines={3} style={[styles.heard, { color: theme.colors.textMuted }]} accessibilityLiveRegion="polite">
            {dictation.heard}
          </Text>
        ) : null}
        {/* Always editable: a non-editable field drops the keyboard on iOS. */}
        <TextInput
          value={value}
          onChangeText={onChangeText}
          placeholder={placeholder}
          placeholderTextColor={theme.colors.textSubtle}
          selectionColor={caretColors.selection}
          cursorColor={caretColors.caret}
          selectionHandleColor={caretColors.caret}
          underlineColorAndroid="transparent"
          keyboardAppearance={isDarkBackground(theme.colors.background) ? "dark" : "light"}
          accessibilityLabel={accessibilityLabel}
          {...(accessibilityHint ? { accessibilityHint } : {})}
          multiline
          allowFontScaling
          {...inputProps}
          style={[styles.input, { color: theme.colors.text }]}
        />
        <View style={styles.controls}>
          {leading}
          {bottom}
          <View style={styles.spacer} />
        {dictating ? (
          <RecordingPill phase={dictation.phase} startedAt={dictation.startedAt} levels={dictation.levels} onStop={dictation.stop} />
        ) : !sending ? (
          <Control
            accessibilityRole="button"
            accessibilityLabel="Dictate"
            onPress={() => {
              dictation.clearError();
              void dictation.start();
            }}
            hitSlop={4}
            style={({ pressed }) => [styles.control, pressed ? { backgroundColor: theme.colors.surface } : null]}
          >
            <Mic size={20} strokeWidth={1.75} color={theme.colors.textMuted} />
          </Control>
        ) : null}
        {showAction ? (
        <Control
          accessibilityRole="button"
          accessibilityLabel={stopMode ? "Stop" : sending ? "Sending" : "Send"}
          accessibilityState={{ disabled: !enabled, busy: spinning }}
          disabled={!enabled}
          onPress={stopMode ? onStop : onSend}
          hitSlop={6}
          style={({ pressed }) => [
            styles.control,
            { backgroundColor: solid && !spinning ? theme.colors.text : theme.colors.surface },
            pressed ? styles.pressed : null,
          ]}
        >
          {spinning ? (
            <ActivityIndicator size="small" color={theme.colors.textMuted} />
          ) : stopMode ? (
            <View style={[styles.stopGlyph, { backgroundColor: theme.colors.background }]} />
          ) : (
            <ArrowUp size={19} strokeWidth={2.4} color={solid ? theme.colors.background : theme.colors.textSubtle} />
          )}
        </Control>
        ) : null}
        </View>
      </View>
    </View>
  );
}

/**
 * Recording, in the message box: a level meter that moves with your voice,
 * how long it has run, and the button that stops it.
 */
function RecordingPill({ phase, startedAt, levels, onStop }: {
  phase: DictationPhase;
  startedAt: number | null;
  levels: readonly number[];
  onStop: () => void;
}) {
  const { theme } = useTheme();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (phase !== "recording") return;
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, [phase]);
  const live = phase === "recording";
  const bars = Array.from({ length: METER_BARS }, (_, index) => levels[levels.length - METER_BARS + index] ?? 0);
  return (
    <View style={[styles.recording, { backgroundColor: theme.colors.surface }]}>
      {live ? (
        <View style={styles.meter} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
          {bars.map((level, index) => (
            <View key={index} style={[styles.bar, { height: 4 + level * 14, backgroundColor: theme.colors.accent }]} />
          ))}
        </View>
      ) : (
        <ActivityIndicator size="small" color={theme.colors.textMuted} />
      )}
      <Text style={[styles.recordingTime, { color: theme.colors.text }]}>
        {live && startedAt !== null ? recordingTime(now - startedAt) : phase === "finalizing" ? "…" : "0:00"}
      </Text>
      <Control
        accessibilityRole="button"
        accessibilityLabel="Stop dictating"
        onPress={onStop}
        hitSlop={6}
        style={[styles.recordStop, { backgroundColor: theme.colors.danger }]}
      >
        <View style={[styles.recordStopGlyph, { backgroundColor: "#fff" }]} />
      </Control>
    </View>
  );
}

/** A floating note over the conversation ("needs your answer", "Latest"). */
export function FloatingPill({ label, onPress, tone = "neutral", icon }: {
  label: string;
  onPress: () => void;
  tone?: "neutral" | "attention";
  icon?: IconName;
}) {
  const { theme } = useTheme();
  const attention = tone === "attention";
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      style={({ pressed }) => [
        styles.floatPill,
        {
          backgroundColor: attention ? theme.colors.accent : theme.colors.surfaceRaised,
          borderColor: attention ? theme.colors.accent : theme.colors.border,
          shadowColor: "#000",
        },
        pressed ? styles.pressed : null,
      ]}
    >
      {icon ? <Icon name={icon} size={15} tone={attention ? "onAccent" : "textMuted"} /> : null}
      <AppText variant="caption" style={[styles.strong, { color: attention ? theme.colors.onAccent : theme.colors.text }]}>{label}</AppText>
    </Control>
  );
}

const styles = StyleSheet.create({
  strong: { fontWeight: "600" },
  pressed: { opacity: 0.7 },
  dock: { position: "absolute", left: 0, right: 0, bottom: 0, paddingHorizontal: 12, paddingBottom: 8, gap: 6 },
  inlineDock: { paddingHorizontal: 12, paddingTop: 6, paddingBottom: 8, gap: 6 },
  errorPill: { alignSelf: "center", borderWidth: StyleSheet.hairlineWidth, borderRadius: 999, paddingHorizontal: 12, paddingVertical: 6 },
  pill: {
    padding: 7,
    gap: 2,
    borderRadius: 26,
    borderWidth: StyleSheet.hairlineWidth,
    shadowOpacity: 0.1,
    shadowRadius: 6,
    shadowOffset: { width: 0, height: 2 },
    elevation: 2,
  },
  controls: { flexDirection: "row", alignItems: "center", gap: 6 },
  spacer: { flex: 1 },
  heard: { fontSize: 14, lineHeight: 19, fontStyle: "italic", paddingHorizontal: 10, paddingTop: 4 },
  recording: { flexDirection: "row", alignItems: "center", gap: 8, height: CONTROL, borderRadius: CONTROL / 2, paddingLeft: 12, paddingRight: 3 },
  meter: { flexDirection: "row", alignItems: "center", gap: 3, height: 18 },
  bar: { width: 3, borderRadius: 2 },
  recordingTime: { fontSize: 13, fontVariant: ["tabular-nums"], minWidth: 30 },
  recordStop: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center" },
  recordStopGlyph: { width: 11, height: 11, borderRadius: 2 },
  input: {
    alignSelf: "stretch",
    minWidth: 0,
    fontSize: 17,
    lineHeight: 22,
    minHeight: CONTROL,
    maxHeight: 150,
    paddingHorizontal: 8,
    paddingTop: Platform.OS === "ios" ? 9 : 7,
    paddingBottom: Platform.OS === "ios" ? 9 : 7,
    textAlignVertical: "center",
  },
  control: { width: CONTROL, height: CONTROL, borderRadius: CONTROL / 2, alignItems: "center", justifyContent: "center" },
  stopGlyph: { width: 13, height: 13, borderRadius: 3 },
  floatPill: {
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: 999,
    paddingHorizontal: 14,
    minHeight: 36,
    shadowOpacity: 0.1,
    shadowRadius: 6,
    shadowOffset: { width: 0, height: 2 },
    elevation: 2,
  },
});
