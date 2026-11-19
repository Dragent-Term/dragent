/**
 * The views for each transcript line: your message bubble, the turn timer,
 * replies, step lines joined to a thin guide line on the left, the tally
 * that groups a run of steps, and the changed-files card. Text for work in
 * progress shimmers.
 */
import { memo, useEffect, useMemo, useRef, useState, type ComponentType, type ReactElement, type ReactNode } from "react";
import { Animated, Easing, Platform, StyleSheet, Text, View, type StyleProp, type TextStyle, type ViewStyle } from "react-native";
import MaskedViewModule from "@react-native-masked-view/masked-view";
import { LinearGradient } from "expo-linear-gradient";
import * as Clipboard from "expo-clipboard";
import {
  Bot, Check, ChevronRight, CircleDashed, Copy, FileDiff, FileText, FileX, Globe, ListTodo, PencilLine, Search,
  Sparkles, SquareTerminal, Wrench,
} from "lucide-react-native";
import { Control } from "../../components";
import { statusMarkColors, useTheme } from "../../theme";
import { Markdown } from "../agent/AgentMarkdown";
import { clockLabel, dominantKind, runSummary, runningTime, type ChangedFile, type Line, type Step, type StepKind, type YouNote } from "./transcript";

// The module's class typing predates this React's JSX types.
const MaskedView = MaskedViewModule as unknown as ComponentType<{
  style?: StyleProp<ViewStyle>;
  maskElement: ReactElement;
  children: ReactNode;
}>;

const MONO = Platform.select({ ios: "Menlo", android: "monospace", default: "monospace" });
/** The newest steps a working run keeps in view while folded. */
const ACTIVE_TAIL = 3;

// ---- Colour: the desktop's "foreground at N%" over the background ----

function rgb(color: string): [number, number, number, number] | null {
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(color.trim());
  if (hex) {
    const body = hex[1]!.length === 3 ? hex[1]!.split("").map((c) => c + c).join("") : hex[1]!;
    return [parseInt(body.slice(0, 2), 16), parseInt(body.slice(2, 4), 16), parseInt(body.slice(4, 6), 16), 1];
  }
  const fn = /^rgba?\(([^)]+)\)$/i.exec(color.trim());
  if (fn) {
    const parts = fn[1]!.split(",").map((part) => parseFloat(part));
    if (parts.length >= 3 && parts.every((part) => Number.isFinite(part))) return [parts[0]!, parts[1]!, parts[2]!, parts[3] ?? 1];
  }
  return null;
}

/** `amount` of `fg` over `bg`, as an opaque colour. */
export function mix(fg: string, bg: string, amount: number): string {
  const a = rgb(fg);
  const b = rgb(bg);
  if (!a || !b) return fg;
  const t = amount * a[3];
  const channel = (i: 0 | 1 | 2) => Math.round(a[i] * t + b[i] * (1 - t));
  return `rgb(${channel(0)}, ${channel(1)}, ${channel(2)})`;
}

function alpha(color: string, amount: number): string {
  const c = rgb(color);
  return c ? `rgba(${c[0]}, ${c[1]}, ${c[2]}, ${amount})` : color;
}

/** The transcript's palette, from the theme. */
export function useInk() {
  const { theme } = useTheme();
  const { text, background } = theme.colors;
  return useMemo(() => {
    const marks = statusMarkColors(theme.colors);
    return {
      text,
      background,
      quiet: mix(text, background, 0.5),
      icon: mix(text, background, 0.45),
      rail: mix(text, background, 0.14),
      chip: mix(text, background, 0.06),
      chipText: mix(text, background, 0.7),
      bubble: mix(text, background, 0.1),
      faint: mix(text, background, 0.6),
      header: mix(text, background, 0.03),
      border: theme.colors.border,
      added: marks.success,
      removed: marks.danger,
      danger: marks.danger,
    };
  }, [text, background, theme.colors]);
}

/** The current time, every second while `active`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    setNow(Date.now());
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  return now;
}

// ---- Live text ----

/** One line of text with a highlight sweeping across it, for what is still happening. */
export function Shimmer({ children, style, duration = 1.6 }: { children: string; style?: StyleProp<TextStyle>; duration?: number }) {
  const ink = useInk();
  const [width, setWidth] = useState(0);
  const sweep = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    if (width === 0) return;
    sweep.setValue(0);
    const loop = Animated.loop(Animated.timing(sweep, { toValue: 1, duration: duration * 1000, easing: Easing.linear, useNativeDriver: true }));
    loop.start();
    return () => loop.stop();
  }, [width, duration, sweep]);
  const band = Math.max(36, width * 0.45);
  const translateX = sweep.interpolate({ inputRange: [0, 1], outputRange: [-band, width + band] });
  const text = <Text numberOfLines={1} style={style}>{children}</Text>;
  return (
    <MaskedView style={styles.shrink} maskElement={text}>
      <View style={{ backgroundColor: ink.quiet }} onLayout={(event) => setWidth(event.nativeEvent.layout.width)}>
        <Text numberOfLines={1} style={[style, styles.invisible]}>{children}</Text>
        {width > 0 ? (
          <Animated.View pointerEvents="none" style={[styles.band, { width: band, transform: [{ translateX }] }]}>
            <LinearGradient
              colors={[alpha(ink.text, 0), alpha(ink.text, 1), alpha(ink.text, 0)]}
              start={{ x: 0, y: 0.5 }}
              end={{ x: 1, y: 0.5 }}
              style={styles.fill}
            />
          </Animated.View>
        ) : null}
      </View>
    </MaskedView>
  );
}

/** Spinning dashed circle shown beside a step that hasn't finished. */
function Spinning({ size = 14 }: { size?: number }) {
  const ink = useInk();
  const turn = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    const loop = Animated.loop(Animated.timing(turn, { toValue: 1, duration: 2400, easing: Easing.linear, useNativeDriver: true }));
    loop.start();
    return () => loop.stop();
  }, [turn]);
  const rotate = turn.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "360deg"] });
  return (
    <Animated.View style={{ transform: [{ rotate }] }}>
      <CircleDashed size={size} strokeWidth={1.75} color={ink.icon} />
    </Animated.View>
  );
}

const KIND_ICONS: Record<StepKind, typeof FileText> = {
  read: FileText,
  edit: PencilLine,
  delete: FileX,
  search: Search,
  execute: SquareTerminal,
  fetch: Globe,
  think: Sparkles,
  agent: Bot,
  plan: ListTodo,
  other: Wrench,
};

// ---- The rail ----

/**
 * Wraps one step in its guide-line geometry: a vertical line down the left
 * edge plus a small rounded elbow pointing at the step's text. For the final
 * step the vertical line stops at the elbow instead of running on.
 */
export function StepGuide({ last, children }: { last: boolean; children: ReactNode }) {
  const ink = useInk();
  return (
    <View style={styles.stepGuide}>
      <View pointerEvents="none" style={[styles.guide, last ? styles.guideEnd : null, { backgroundColor: ink.rail }]} />
      <View pointerEvents="none" style={[styles.elbow, { borderColor: ink.rail }]} />
      {children}
    </View>
  );
}

function DiffTally({ added, removed }: { added: number | null; removed: number | null }) {
  const ink = useInk();
  if (added === null && removed === null) return null;
  return (
    <Text style={styles.tally}>
      <Text style={{ color: ink.added }}>+{added ?? 0}</Text>
      <Text style={{ color: ink.removed }}> −{removed ?? 0}</Text>
    </Text>
  );
}

/**
 * One tool call. Shows the action word, a file name chip or the command in
 * monospace, and +/− line counts for edits. Unfinished steps get the spinner,
 * a shimmer and an elapsed-seconds counter; finished ones are drawn faded.
 * Tapping shows the full command or path.
 */
export const StepLine = memo(function StepLine({ step }: { step: Step }) {
  const ink = useInk();
  const [open, setOpen] = useState(false);
  const now = useNow(step.running && step.startedAt !== null);
  const detail = step.detail ?? (step.label.length > 48 ? step.label : null);
  const words = [step.verb, step.file ? null : step.label].filter(Boolean).join(" ");
  const textStyle = [styles.stepText, { color: step.running ? ink.text : ink.quiet }];
  return (
    <View>
      <Control
        accessibilityRole="button"
        accessibilityState={{ expanded: open, disabled: !detail }}
        accessibilityLabel={`${[step.verb, step.file, step.label].filter(Boolean).join(" ")}${step.failed ? ", failed" : ""}`}
        disabled={!detail}
        onPress={() => setOpen((value) => !value)}
        style={styles.stepRow}
      >
        {step.running ? <Spinning /> : null}
        {step.running ? (
          <Shimmer style={[styles.stepText, step.mono && !step.verb ? styles.monoText : null]}>{words || step.label}</Shimmer>
        ) : step.verb ? (
          <Text style={textStyle} numberOfLines={1}>{step.verb}</Text>
        ) : null}
        {step.file ? (
          <View style={[styles.chip, { backgroundColor: ink.chip }]}>
            <Text numberOfLines={1} style={[styles.chipText, { color: ink.chipText }]}>{step.file}</Text>
          </View>
        ) : !step.running && step.label ? (
          <Text numberOfLines={1} style={[textStyle, styles.shrink, step.mono ? styles.monoText : null]}>{step.label}</Text>
        ) : null}
        {!step.running ? <DiffTally added={step.added} removed={step.removed} /> : null}
        {step.running && step.startedAt !== null ? (
          <Text style={[styles.status, { color: ink.faint }]}>{runningTime(step.startedAt, now)}</Text>
        ) : null}
        {step.failed ? <Text style={[styles.status, { color: ink.danger }]}>failed</Text> : null}
      </Control>
      {open && detail ? (
        <View style={[styles.stepDetail, { borderColor: ink.border }]}>
          <Text selectable style={[styles.detailText, { color: ink.quiet }]}>{detail}</Text>
        </View>
      ) : null}
    </View>
  );
});

/**
 * Consecutive steps under one line: "Ran 6 commands, edited 3 files". Folded
 * by default; while the agent works on them the newest few hang below it.
 */
export const RunLine = memo(function RunLine({ steps, active, open, onToggle }: {
  steps: readonly Step[];
  active: boolean;
  open: boolean;
  onToggle: () => void;
}) {
  const ink = useInk();
  const done = steps.filter((step) => !step.running);
  const live = done.length === 0;
  const summary = runSummary(live ? steps : done);
  const Glyph = KIND_ICONS[dominantKind(steps)];
  const shown = open ? steps : active ? steps.slice(-ACTIVE_TAIL) : [];
  return (
    <View>
      <Control
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        accessibilityLabel={`${summary}. ${open ? "Hide" : "Show"} the steps.`}
        onPress={onToggle}
        style={styles.runHead}
      >
        <Glyph size={14} strokeWidth={1.75} color={ink.icon} />
        {live ? (
          <Shimmer style={styles.stepText}>{summary}</Shimmer>
        ) : (
          <Text numberOfLines={1} style={[styles.stepText, styles.shrink, { color: ink.quiet }]}>{summary}</Text>
        )}
        <View style={open ? styles.rotated : null}>
          <ChevronRight size={14} strokeWidth={1.75} color={ink.icon} />
        </View>
      </Control>
      {shown.map((step, index) => (
        <StepGuide key={step.id} last={index === shown.length - 1}>
          <StepLine step={step} />
        </StepGuide>
      ))}
    </View>
  );
});

/** The turn timer line ("Working · 12s", then "Done in 1m 3s"). */
export function ClockLine({ line }: { line: Extract<Line<unknown>, { kind: "clock" }> }) {
  const ink = useInk();
  const ticking = line.running && !line.waiting;
  const now = useNow(ticking);
  const label = clockLabel(line, now);
  if (!label) return null;
  return (
    <View style={styles.clock}>
      {ticking ? (
        <Shimmer style={styles.stepText} duration={1}>{label}</Shimmer>
      ) : (
        <Text style={[styles.stepText, { color: ink.quiet }]}>{label}</Text>
      )}
    </View>
  );
}

/** Your message: a soft bubble on the right, round while it's one line. */
export const YouLine = memo(function YouLine({ text, note, onRetry, onDiscard, onCancel }: {
  text: string;
  note: YouNote | null;
  onRetry?: () => void;
  onDiscard?: () => void;
  onCancel?: () => void;
}) {
  const ink = useInk();
  const [oneLine, setOneLine] = useState(true);
  const settling = note !== null && note.kind !== "failed";
  return (
    <View style={styles.you}>
      <View style={[styles.bubble, { backgroundColor: ink.bubble, borderRadius: oneLine ? 999 : 14 }, settling ? styles.dim : null]}>
        <Text
          selectable
          onTextLayout={(event) => setOneLine(event.nativeEvent.lines.length <= 1)}
          style={[styles.body, { color: ink.text }]}
        >
          {text}
        </Text>
      </View>
      {note ? (
        <View style={styles.youNote}>
          {note.kind === "failed" ? (
            <>
              <Text style={[styles.noteText, { color: ink.danger }]}>Not delivered</Text>
              {onRetry ? <Text style={[styles.noteAction, { color: ink.text }]} onPress={onRetry} accessibilityRole="button">Retry</Text> : null}
              {onDiscard ? <Text style={[styles.noteAction, { color: ink.quiet }]} onPress={onDiscard} accessibilityRole="button">Discard</Text> : null}
            </>
          ) : (
            <>
              <Text style={[styles.noteText, { color: ink.quiet }]}>{note.kind === "queued" ? "Queued" : "Sending…"}</Text>
              {onCancel ? <Text style={[styles.noteAction, { color: ink.quiet }]} onPress={onCancel} accessibilityRole="button">Cancel</Text> : null}
            </>
          )}
        </View>
      ) : null}
    </View>
  );
});

function clockTime(at: number): string {
  const date = new Date(at);
  const hours = date.getHours();
  return `${hours % 12 === 0 ? 12 : hours % 12}:${String(date.getMinutes()).padStart(2, "0")} ${hours < 12 ? "AM" : "PM"}`;
}

/** Copy, and the time of the reply, under the turn's last reply. */
function ReplyActions({ text, at }: { text: string; at: number | null }) {
  const ink = useInk();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);
  const Glyph = copied ? Check : Copy;
  return (
    <View style={styles.footer}>
      <Control
        accessibilityRole="button"
        accessibilityLabel={copied ? "Copied" : "Copy message"}
        hitSlop={8}
        onPress={() => void Clipboard.setStringAsync(text).then(() => setCopied(true), () => {})}
        style={styles.footerButton}
      >
        <Glyph size={14} strokeWidth={1.75} color={copied ? ink.text : ink.faint} />
      </Control>
      {at !== null ? <Text style={[styles.noteText, { color: ink.faint }]}>{clockTime(at)}</Text> : null}
    </View>
  );
}

/** The agent's reply, in full. */
export const ReplyLine = memo(function ReplyLine({ text, lastReply, at }: { text: string; lastReply: boolean; at: number | null }) {
  return (
    <View style={styles.reply}>
      <Markdown text={text} reading />
      {lastReply ? <ReplyActions text={text} at={at} /> : null}
    </View>
  );
});

/** The files a settled turn changed, with what each cost. */
export const FilesTouchedCard = memo(function FilesTouchedCard({ files }: { files: readonly ChangedFile[] }) {
  const ink = useInk();
  const added = files.reduce((sum, file) => sum + file.added, 0);
  const removed = files.reduce((sum, file) => sum + file.removed, 0);
  return (
    <View style={[styles.card, { borderColor: ink.border }]}>
      <View style={[styles.cardHead, { backgroundColor: ink.header, borderColor: ink.border }]}>
        <FileDiff size={14} strokeWidth={1.75} color={ink.icon} />
        <Text style={[styles.cardTitle, { color: ink.text }]} numberOfLines={1}>
          {files.length} file{files.length === 1 ? "" : "s"} changed
        </Text>
        <DiffTally added={added} removed={removed} />
      </View>
      {files.map((file, index) => (
        <View key={file.path} style={[styles.cardRow, index > 0 ? { borderTopWidth: StyleSheet.hairlineWidth, borderColor: ink.border } : null]}>
          <Text numberOfLines={1} style={[styles.cardFile, { color: ink.chipText }]}>{file.name}</Text>
          <DiffTally added={file.added} removed={file.removed} />
        </View>
      ))}
    </View>
  );
});

const styles = StyleSheet.create({
  shrink: { flexShrink: 1, minWidth: 0 },
  invisible: { opacity: 0 },
  band: { position: "absolute", top: 0, bottom: 0, left: 0 },
  fill: { flex: 1 },
  rotated: { transform: [{ rotate: "90deg" }] },
  dim: { opacity: 0.6 },
  body: { fontSize: 15, lineHeight: 22 },
  stepText: { fontSize: 14, lineHeight: 20 },
  monoText: { fontFamily: MONO, fontSize: 13 },
  status: { fontFamily: MONO, fontSize: 11, lineHeight: 20 },
  tally: { fontFamily: MONO, fontSize: 12, lineHeight: 20 },
  stepGuide: { paddingLeft: 20 },
  guide: { position: "absolute", left: 6, top: 0, bottom: 0, width: 1 },
  guideEnd: { bottom: undefined, height: 7 },
  elbow: { position: "absolute", left: 6, top: 6, width: 8, height: 8, borderLeftWidth: 1, borderBottomWidth: 1, borderBottomLeftRadius: 8 },
  stepRow: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 28, paddingVertical: 4, minWidth: 0 },
  chip: { borderRadius: 4, paddingHorizontal: 4, paddingVertical: 1, flexShrink: 1, minWidth: 0 },
  chipText: { fontFamily: MONO, fontSize: 13, lineHeight: 18 },
  stepDetail: { marginTop: 2, marginBottom: 8, marginLeft: 7, borderLeftWidth: 1, paddingLeft: 12 },
  detailText: { fontFamily: MONO, fontSize: 12, lineHeight: 17 },
  runHead: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 28, paddingVertical: 4, minWidth: 0 },
  clock: { minHeight: 28, paddingVertical: 4, justifyContent: "center" },
  you: { alignItems: "flex-end", gap: 4 },
  bubble: { maxWidth: "85%", paddingHorizontal: 14, paddingVertical: 8 },
  youNote: { flexDirection: "row", alignItems: "center", gap: 10, paddingRight: 4 },
  noteText: { fontSize: 12, lineHeight: 16 },
  noteAction: { fontSize: 12, lineHeight: 16, fontWeight: "600" },
  reply: { gap: 4, minWidth: 0 },
  footer: { flexDirection: "row", alignItems: "center", gap: 8, marginLeft: -4 },
  footerButton: { padding: 4, borderRadius: 4 },
  card: { marginTop: 4, marginBottom: 8, borderWidth: StyleSheet.hairlineWidth, borderRadius: 8, overflow: "hidden" },
  cardHead: { flexDirection: "row", alignItems: "center", gap: 8, paddingVertical: 6, paddingLeft: 12, paddingRight: 10, borderBottomWidth: StyleSheet.hairlineWidth },
  cardTitle: { flex: 1, minWidth: 0, fontSize: 13, lineHeight: 18 },
  cardRow: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 12, paddingVertical: 7 },
  cardFile: { flex: 1, minWidth: 0, fontFamily: MONO, fontSize: 12.5, lineHeight: 17 },
});

/** What a line needs from the screen around it. */
export interface LineHandlers<C> {
  readonly isOpen: (key: string) => boolean;
  readonly onToggle: (key: string) => void;
  readonly renderCard: (card: C) => ReactNode;
  readonly onRetry?: (id: string) => void;
  readonly onDiscard?: (id: string) => void;
  readonly onCancel?: (id: string) => void;
}

/** Any line of a conversation, spaced the way the desktop spaces it. */
/**
 * One line of the conversation. A re-read of the conversation rebuilds every
 * line object; a line whose content didn't change keeps its rendering (its
 * Markdown is the costly part), so a long chat stays smooth while it updates.
 */
export const TranscriptLine = memo(TranscriptLineView, (before, after) =>
  before.handlers === after.handlers && sameLine(before.line, after.line),
) as <C>(props: { line: Line<C>; handlers: LineHandlers<C> }) => ReactElement | null;

const signatures = new WeakMap<object, string>();

function signature(line: object): string {
  let known = signatures.get(line);
  if (known === undefined) {
    try {
      known = JSON.stringify(line);
    } catch {
      known = String(Math.random());
    }
    signatures.set(line, known);
  }
  return known;
}

function sameLine(a: object, b: object): boolean {
  return a === b || signature(a) === signature(b);
}

function TranscriptLineView<C>({ line, handlers }: { line: Line<C>; handlers: LineHandlers<C> }) {
  switch (line.kind) {
    case "you": {
      const { id } = line;
      return (
        <View style={[lineStyles.row, line.turnStart ? lineStyles.turnStart : null]}>
          <YouLine
            text={line.text}
            note={line.note}
            {...(handlers.onRetry ? { onRetry: () => handlers.onRetry!(id) } : {})}
            {...(handlers.onDiscard ? { onDiscard: () => handlers.onDiscard!(id) } : {})}
            {...(handlers.onCancel && line.note?.kind === "queued" ? { onCancel: () => handlers.onCancel!(id) } : {})}
          />
        </View>
      );
    }
    case "clock":
      return <View style={lineStyles.row}><ClockLine line={line} /></View>;
    case "reply":
      return (
        <View style={[lineStyles.row, line.afterWork ? lineStyles.replyAfterWork : lineStyles.replyTop]}>
          <ReplyLine text={line.text} lastReply={line.lastReply} at={line.at} />
        </View>
      );
    case "run":
      return (
        <View style={lineStyles.row}>
          <RunLine steps={line.steps} active={line.active} open={handlers.isOpen(line.key)} onToggle={() => handlers.onToggle(line.key)} />
        </View>
      );
    case "step":
      return (
        <View style={lineStyles.row}>
          {line.step.kind === "think" ? <StepLine step={line.step} /> : <StepGuide last><StepLine step={line.step} /></StepGuide>}
        </View>
      );
    case "changes":
      return <View style={lineStyles.cardRowOuter}><FilesTouchedCard files={line.files} /></View>;
    case "card":
      return <View style={[lineStyles.row, lineStyles.cardSpace]}>{handlers.renderCard(line.card)}</View>;
  }
}

const lineStyles = StyleSheet.create({
  row: { paddingHorizontal: 16, paddingBottom: 4 },
  turnStart: { paddingTop: 16 },
  replyTop: { paddingTop: 12 },
  replyAfterWork: { paddingTop: 4 },
  cardRowOuter: { paddingHorizontal: 16 },
  cardSpace: { paddingVertical: 6 },
});
