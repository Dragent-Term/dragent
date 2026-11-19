import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Platform, ScrollView, StyleSheet, View, useWindowDimensions } from "react-native";
import { AppText, Button, Control, EmptyState, Icon, SelectPopover, type SelectPopoverOption } from "../../components";
import { rgbaFor, useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";

export interface DiffFile {
  path: string;
  added: number;
  removed: number;
  status?: "added" | "modified" | "deleted";
  previousPath?: string | null;
  lines: readonly { kind: "context" | "add" | "remove"; text: string }[];
}

export interface DiffLineTarget {
  path: string;
  /** Null targets the whole file; otherwise an index into `lines`. */
  lineIndex: number | null;
  /** Optional range end for line-range drafts. */
  lineEnd?: number | null;
}

interface ReviewDiffViewProps {
  files: readonly DiffFile[];
  selectedPath: string | null;
  onSelectPath: (path: string) => void;
  isViewed: (path: string) => boolean;
  onToggleViewed: (path: string, viewed: boolean) => void;
  hideViewed: boolean;
  onToggleHideViewed: () => void;
  notesForPath: (path: string) => readonly { id: string; body: string }[];
  /** Null renders a read-only diff with no line affordances. */
  onLinePress: ((path: string, lineIndex: number | null) => void) | null;
  pendingTarget: DiffLineTarget | null;
  /** Parent-owned composer; rendered only when the parent shows it. */
  composer: ReactNode;
  hint: string;
  emptyMessage: string;
  /** Split (Original | Proposed) vs unified. Effective on wide panes only. */
  split?: boolean | undefined;
  onToggleSplit?: (() => void) | undefined;
  /** Pinned commit sha for read-only commit comparison. */
  commitSha?: string | null | undefined;
  onClearCommit?: (() => void) | undefined;
  /** Open the full file on the provider (preview shows feedback). */
  onOpenFile?: ((path: string) => void) | undefined;
  /** Optional commit scope selector, rendered inline in the toolbar chip row. */
  compareOptions?: readonly SelectPopoverOption<string>[] | undefined;
  compareValue?: string | undefined;
  onSelectCompare?: ((id: string) => void) | undefined;
}

const DIFF_LIMIT = 500;

function lineNumbers(lines: readonly DiffFile["lines"][number][]): { old: (number | null)[]; next: (number | null)[] } {
  let old = 0;
  let next = 0;
  const oldNums: (number | null)[] = [];
  const nextNums: (number | null)[] = [];
  for (const line of lines) {
    if (line.kind === "add") {
      next += 1;
      oldNums.push(null);
      nextNums.push(next);
    } else if (line.kind === "remove") {
      old += 1;
      oldNums.push(old);
      nextNums.push(null);
    } else {
      old += 1;
      next += 1;
      oldNums.push(old);
      nextNums.push(next);
    }
  }
  return { old: oldNums, next: nextNums };
}

function ToggleChip({ label, active, onPress }: { label: string; active: boolean; onPress: () => void }) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole="checkbox"
      accessibilityState={{ checked: active }}
      accessibilityLabel={label}
      onPress={onPress}
      style={({ pressed, hovered }) => [
        styles.chip,
        {
          borderColor: active ? theme.colors.accent : theme.colors.border,
          backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : undefined,
        },
      ]}
    >
      <AppText variant="caption" tone={active ? "text" : "textMuted"}>{label}</AppText>
    </Control>
  );
}

function FileButton({ path, title, selected, viewed, added, removed, onPress }: {
  path: string; title: string; selected: boolean; viewed: boolean;
  added?: number | undefined; removed?: number | undefined; onPress: () => void;
}) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={`${viewed ? "Viewed, " : ""}${path}`}
      accessibilityState={{ selected }}
      onPress={onPress}
      style={({ pressed, hovered }) => [
        styles.fileButton,
        {
          backgroundColor: selected ? theme.colors.surfaceRaised
            : pressed || hovered ? theme.colors.surface : undefined,
        },
      ]}
    >
      {viewed ? <Icon name="check" size={12} tone="success" /> : null}
      <AppText variant="caption" tone={selected ? "text" : "textMuted"} numberOfLines={1} style={styles.fileTitle}>
        {title}
      </AppText>
      {added !== undefined || removed !== undefined ? (
        <View style={styles.fileCounts} accessible accessibilityLabel={`${added ?? 0} additions, ${removed ?? 0} deletions`}>
          <AppText variant="caption" tone="success" style={styles.fileCountNum}>{`+${added ?? 0}`}</AppText>
          <AppText variant="caption" tone="danger" style={styles.fileCountNum}>{`−${removed ?? 0}`}</AppText>
        </View>
      ) : null}
    </Control>
  );
}

/** Collapsible directory tree for the file navigator, mirroring desktop Files. */
function FileTree({ files, selectedPath, isViewed, onSelectPath }: {
  files: readonly DiffFile[];
  selectedPath: string | null;
  isViewed: (path: string) => boolean;
  onSelectPath: (path: string) => void;
}) {
  const [collapsed, setCollapsed] = useState<readonly string[]>([]);
  const counts = useMemo(() => {
    const map = new Map<string, DiffFile>();
    for (const file of files) map.set(file.path, file);
    return map;
  }, [files]);
  const renderLevel = (parent: string, depth: number): ReactNode => {
    const prefix = parent ? `${parent}/` : "";
    const directories = [...new Set(files.map((file) => {
      const rest = file.path.startsWith(prefix) ? file.path.slice(prefix.length) : null;
      const slash = rest?.indexOf("/") ?? -1;
      return slash >= 0 ? rest?.slice(0, slash) ?? null : null;
    }).filter((entry): entry is string => entry !== null))].sort();
    const nodes: ReactNode[] = [];
    for (const directory of directories) {
      const full = `${prefix}${directory}`;
      const closed = collapsed.includes(full);
      nodes.push(
        <Control
          key={full}
          accessibilityRole="button"
          accessibilityLabel={`${closed ? "Expand" : "Collapse"} ${full}`}
          accessibilityState={{ expanded: !closed }}
          onPress={() => setCollapsed(closed ? collapsed.filter((entry) => entry !== full) : [...collapsed, full])}
          style={[styles.treeRow, { paddingLeft: 8 + depth * 12 }]}
        >
          <Icon name={closed ? "chevron-right" : "chevron-down"} size={14} tone="textMuted" />
          <Icon name="folder" size={14} tone="textMuted" />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.treeDirLabel}>{directory}</AppText>
        </Control>,
      );
      if (!closed) nodes.push(<View key={`${full}:children`}>{renderLevel(full, depth + 1)}</View>);
    }
    for (const file of files.filter((entry) => entry.path.startsWith(prefix) && !entry.path.slice(prefix.length).includes("/"))) {
      const leaf = file.path.slice(prefix.length);
      const meta = counts.get(file.path);
      nodes.push(
        <View key={file.path} style={{ paddingLeft: depth * 12 }}>
          <FileButton path={file.path} title={leaf} selected={file.path === selectedPath}
            viewed={isViewed(file.path)} added={meta?.added} removed={meta?.removed}
            onPress={() => onSelectPath(file.path)} />
        </View>,
      );
    }
    return <>{nodes}</>;
  };
  return <View>{renderLevel("", 0)}</View>;
}

function isHunkLine(line: DiffFile["lines"][number]): boolean {
  return line.kind === "context" && line.text.trimStart().startsWith("@@");
}

function diffLineBackground(
  theme: ReturnType<typeof useTheme>["theme"],
  kind: DiffFile["lines"][number]["kind"],
  hunk: boolean,
  active: boolean,
): string | undefined {
  if (active) return theme.colors.selection;
  if (hunk) return rgbaFor(theme.colors.accent, 0.12);
  if (kind === "add") return rgbaFor(theme.colors.success, 0.12);
  if (kind === "remove") return rgbaFor(theme.colors.danger, 0.12);
  return undefined;
}

function inRange(pending: DiffLineTarget | null, path: string, lineIndex: number): boolean {
  if (!pending || pending.path !== path) return false;
  if (pending.lineIndex === null) return false;
  const start = pending.lineIndex;
  const end = pending.lineEnd ?? start;
  const [lo, hi] = start <= end ? [start, end] : [end, start];
  return lineIndex >= lo && lineIndex <= hi;
}

function DiffBody({ file, pendingTarget, onLinePress, numbers, header, split, narrow }: {
  header: ReactNode;
  file: DiffFile;
  pendingTarget: DiffLineTarget | null;
  onLinePress: ((path: string, lineIndex: number | null) => void) | null;
  numbers: { old: (number | null)[]; next: (number | null)[] };
  split: boolean;
  narrow: boolean;
}) {
  const { theme } = useTheme();
  const [limit, setLimit] = useState(DIFF_LIMIT);
  const commentable = onLinePress !== null;
  const useSplit = split && !narrow;
  return (
    <View style={styles.diffBody}>
      {header}
      <View style={[styles.diffRule, { backgroundColor: theme.colors.border }]} />
      {useSplit ? (
        <View style={styles.splitHeader}>
          <AppText variant="caption" tone="textSubtle" style={styles.splitTitle}>Original</AppText>
          <AppText variant="caption" tone="textSubtle" style={styles.splitTitle}>Proposed</AppText>
        </View>
      ) : null}
      <ScrollView style={styles.diffScroll} contentContainerStyle={styles.diffContent} keyboardShouldPersistTaps="handled">
        <ScrollView horizontal contentContainerStyle={styles.diffHorizontal} accessibilityLabel={`Diff for ${file.path}`}>
          <View style={styles.diff}>
            {file.lines.slice(0, limit).map((line, lineIndex) => {
              const hunk = isHunkLine(line);
              const ranged = inRange(pendingTarget, file.path, lineIndex);
              const singleActive = pendingTarget?.path === file.path && pendingTarget.lineIndex === lineIndex && (pendingTarget.lineEnd == null);
              const active = ranged || singleActive;
              const tone = hunk ? "accent" : line.kind === "add" ? "success" : line.kind === "remove" ? "danger" : "text";
              const background = diffLineBackground(theme, line.kind, hunk, active);
              const oldNum = numbers.old[lineIndex];
              const newNum = numbers.next[lineIndex];
              const rangeSuffix = pendingTarget?.path === file.path && pendingTarget.lineEnd != null && ranged
                ? ` (lines ${Math.min(pendingTarget.lineIndex ?? 0, pendingTarget.lineEnd) + 1}–${Math.max(pendingTarget.lineIndex ?? 0, pendingTarget.lineEnd) + 1})` : "";
              const label = hunk ? `Hunk header: ${line.text}`
                : `${line.kind === "add" ? "Added" : line.kind === "remove" ? "Removed" : "Context"} line ${newNum ?? oldNum ?? lineIndex + 1}: ${line.text}${rangeSuffix}`;
              if (useSplit) {
                const leftVisible = line.kind !== "add";
                const rightVisible = line.kind !== "remove";
                const leftText = !leftVisible ? "" : hunk ? line.text : `${line.kind === "remove" ? "−" : " "} ${line.text}`;
                const rightText = !rightVisible ? "" : hunk ? line.text : `${line.kind === "add" ? "+" : " "} ${line.text}`;
                const row = (
                  <View style={[styles.splitRow, background ? { backgroundColor: background } : null]}>
                    <View style={styles.splitCell}>
                      <AppText variant="caption" tone="textMuted" style={styles.lineNumber}>
                        {hunk ? "@@" : oldNum != null ? `${oldNum}` : "·"}
                      </AppText>
                      <AppText selectable={!commentable} variant="caption" tone={leftVisible ? tone : "textSubtle"}
                        style={styles.code} accessibilityLabel={commentable ? undefined : label}>
                        {leftText}
                      </AppText>
                    </View>
                    <View style={[styles.splitDivider, { backgroundColor: theme.colors.border }]} />
                    <View style={styles.splitCell}>
                      <AppText variant="caption" tone="textMuted" style={styles.lineNumber}>
                        {hunk ? "@@" : newNum != null ? `${newNum}` : "·"}
                      </AppText>
                      <AppText selectable={!commentable} variant="caption" tone={rightVisible ? tone : "textSubtle"}
                        style={styles.code} accessibilityLabel={commentable ? undefined : label}>
                        {rightText}
                      </AppText>
                    </View>
                  </View>
                );
                return commentable && !hunk ? (
                  <Control key={lineIndex} accessibilityRole="button" accessibilityLabel={`Comment on ${label}`}
                    onPress={() => onLinePress(file.path, lineIndex)}
                    style={[styles.linePress, background ? { backgroundColor: background } : null]}>
                    {row}
                  </Control>
                ) : (
                  <View key={lineIndex} accessible accessibilityLabel={label}>{row}</View>
                );
              }
              const row = (
                <View style={[styles.diffLine, background ? { backgroundColor: background } : null]}>
                  {commentable && !hunk ? (
                    <AppText variant="caption" tone={active ? "accent" : "textSubtle"} style={styles.plus}>+</AppText>
                  ) : null}
                  <AppText variant="caption" tone={hunk ? "accent" : "textMuted"} style={[styles.lineNumber, hunk ? styles.hunkText : null]}>
                    {hunk ? "@@" : `${oldNum ?? ""} ${newNum ?? ""}`.trim() || "·"}
                  </AppText>
                  <AppText
                    selectable={!commentable}
                    variant="caption"
                    tone={tone}
                    style={[styles.code, hunk ? styles.hunkText : null]}
                    accessibilityLabel={commentable ? undefined : label}
                  >
                    {hunk ? line.text : `${line.kind === "add" ? "+" : line.kind === "remove" ? "−" : " "} ${line.text}`}
                  </AppText>
                </View>
              );
              return commentable && !hunk ? (
                <Control
                  key={lineIndex}
                  accessibilityRole="button"
                  accessibilityLabel={`Comment on ${label}`}
                  onPress={() => onLinePress(file.path, lineIndex)}
                  style={[styles.linePress, background ? { backgroundColor: background } : null]}
                >
                  {row}
                </Control>
              ) : (
                <View key={lineIndex} accessible accessibilityLabel={label}>{row}</View>
              );
            })}
          </View>
        </ScrollView>
        {file.lines.length > limit ? (
          <View style={styles.moreRow}>
            <Button label={`Show ${DIFF_LIMIT} more lines`} size="sm" variant="ghost" onPress={() => setLimit(limit + DIFF_LIMIT)} />
          </View>
        ) : null}
      </ScrollView>
    </View>
  );
}

/**
 * Shared unified-diff presentation for hosted request diffs and workspace
 * local diffs. Viewed/draft state stays with the caller, so provider review
 * state and workspace Git state never mix. The note composer is caller-owned
 * and renders only on demand — never an always-visible oversized textarea.
 */
export function ReviewDiffView({
  files, selectedPath, onSelectPath, isViewed, onToggleViewed,
  hideViewed, onToggleHideViewed, notesForPath, onLinePress, pendingTarget, composer, hint, emptyMessage,
  split = false, onToggleSplit, commitSha = null, onClearCommit, onOpenFile,
  compareOptions, compareValue, onSelectCompare,
}: ReviewDiffViewProps) {
  const { theme } = useTheme();
  const window = useWindowDimensions();
  const [tree, setTree] = useState(false);
  const [compareOpen, setCompareOpen] = useState(false);
  const [paneWidth, setPaneWidth] = useState(0);
  // Container width, not the viewport: in the wide inbox split the detail
  // canvas is far narrower than the window, so a viewport breakpoint would
  // force the 220px sidebar into a tiny pane.
  const narrow = (paneWidth > 0 ? paneWidth : window.width) < 700;

  const visible = useMemo(
    () => files.filter((file) => !hideViewed || !isViewed(file.path)).sort((a, b) => a.path.localeCompare(b.path)),
    [files, hideViewed, isViewed],
  );
  const selected = visible.find((file) => file.path === selectedPath) ?? visible[0] ?? null;
  const index = selected ? visible.indexOf(selected) : -1;
  const numbers = useMemo(() => (selected ? lineNumbers(selected.lines) : null), [selected]);
  const notes = useMemo(() => (selected ? notesForPath(selected.path) : []), [selected, notesForPath]);
  const viewed = selected ? isViewed(selected.path) : false;

  // A stale selection (file hidden by Hide viewed, or gone after a refresh)
  // resyncs to a visible file through the parent's selector, which also
  // clears any pending note target so a stale composer never lingers.
  useEffect(() => {
    if (selectedPath !== null && visible.every((file) => file.path !== selectedPath)) {
      const next = visible[0];
      if (next) onSelectPath(next.path);
    }
  }, [selectedPath, visible, onSelectPath]);

  if (!selected || !numbers) {
    const trapped = hideViewed && files.length > 0;
    return (
      <View style={styles.empty}>
        {trapped ? (
          <EmptyState
            title="All files viewed"
            message="Every changed file is marked viewed."
            actionLabel="Show viewed files"
            onAction={onToggleHideViewed}
          />
        ) : (
          <EmptyState title="No files to show" message={emptyMessage} />
        )}
      </View>
    );
  }

  const fileList = tree ? (
    <FileTree files={visible} selectedPath={selected.path} isViewed={isViewed} onSelectPath={onSelectPath} />
  ) : (
    visible.map((file) => (
      <FileButton key={file.path} path={file.path} title={file.path} selected={file.path === selected.path}
        viewed={isViewed(file.path)} added={file.added} removed={file.removed}
        onPress={() => onSelectPath(file.path)} />
    ))
  );

  const compareChip = compareOptions && onSelectCompare ? (
    <SelectPopover label="Compare" caption="COMPARE COMMITS"
      options={compareOptions} value={compareValue ?? ""}
      onChange={(id) => { onSelectCompare(id); setCompareOpen(false); }}
      open={compareOpen} onOpenChange={setCompareOpen} />
  ) : null;

  const navigator = narrow ? (
    <View style={[styles.narrowFiles, { borderColor: theme.colors.border }]}>
      <View style={styles.narrowFilesHeader}>
        <AppText variant="caption" tone="textSubtle" style={styles.sidebarTitle}>
          {`CHANGED FILES · ${index + 1}/${visible.length}`}
        </AppText>
        <View style={styles.chipRow}>
          {compareChip}
          <ToggleChip label="Tree" active={tree} onPress={() => setTree(!tree)} />
          <ToggleChip label="Hide viewed" active={hideViewed} onPress={onToggleHideViewed} />
        </View>
      </View>
      <ScrollView style={styles.narrowFilesScroll} keyboardShouldPersistTaps="handled"
        accessibilityLabel="Changed files">
        {fileList}
      </ScrollView>
    </View>
  ) : (
    <View style={[styles.sidebar, { borderColor: theme.colors.border }]}>
      <AppText variant="caption" tone="textSubtle" style={styles.sidebarTitle}>
        {`CHANGED FILES · ${visible.length}`}
      </AppText>
      <View style={styles.chipRow}>
        {compareChip}
        <ToggleChip label="Tree" active={tree} onPress={() => setTree(!tree)} />
        <ToggleChip label="Hide viewed" active={hideViewed} onPress={onToggleHideViewed} />
      </View>
      <ScrollView style={styles.sidebarScroll} keyboardShouldPersistTaps="handled">
        {fileList}
      </ScrollView>
    </View>
  );

  return (
    <View
      style={styles.frame}
      onLayout={(event) => {
        const next = event.nativeEvent.layout.width;
        setPaneWidth((current) => (Math.abs(current - next) > 1 ? next : current));
      }}
    >
      <View style={[styles.panes, narrow ? styles.panesNarrow : null]}>
        {navigator}
        <View style={styles.diffPane}>
          <DiffBody key={`${selected.path}:${split ? "split" : "unified"}`} file={selected}
            pendingTarget={pendingTarget} onLinePress={commitSha ? null : onLinePress} numbers={numbers}
            split={split} narrow={narrow}
            header={
              <>
              <View style={styles.fileBar}>
                <View style={styles.fileCopy}>
                  <AppText variant="caption" style={styles.bold} selectable numberOfLines={2}>{selected.path}</AppText>
                  <AppText variant="caption" tone="textMuted" numberOfLines={1}>
                    {`${selected.status ?? "changed"} · `}
                    <AppText tone="success">{`+${selected.added}`}</AppText>
                    {` `}
                    <AppText tone="danger">{`−${selected.removed}`}</AppText>
                    {` · ${index + 1}/${visible.length}${viewed ? " · Viewed" : ""}${notes.length > 0 ? ` · ${notes.length} note${notes.length === 1 ? "" : "s"}` : ""}`}
                  </AppText>
                  {selected.previousPath ? (
                    <AppText variant="caption" tone="textSubtle" numberOfLines={1}>
                      {`Renamed from ${selected.previousPath}`}
                    </AppText>
                  ) : null}
                </View>
                {commitSha ? (
                  <View style={styles.commitBanner}>
                    <AppText variant="caption" tone="accent" numberOfLines={2}>
                      {`Commit ${commitSha.slice(0, 8)} · read-only`}
                    </AppText>
                    {onClearCommit ? (
                      <Button label="All changes" size="sm" variant="ghost" onPress={onClearCommit} />
                    ) : null}
                  </View>
                ) : null}
                <View style={styles.fileActions}>
                  {!narrow && onToggleSplit ? (
                    <ToggleChip label={split ? "Split" : "Unified"} active={split} onPress={onToggleSplit} />
                  ) : null}
                  <Button label={viewed ? "✓ Viewed" : "Mark viewed"} size="sm" variant="ghost"
                    onPress={() => onToggleViewed(selected.path, !viewed)} />
                  {onLinePress && !commitSha ? (
                    <Button label="Add note" size="sm" variant="ghost" onPress={() => onLinePress(selected.path, null)} />
                  ) : null}
                  {onOpenFile ? (
                    <Button label="Open full file ↗" size="sm" variant="ghost" onPress={() => onOpenFile(selected.path)} />
                  ) : null}
                </View>
              </View>
              <AppText variant="caption" tone="textSubtle" style={styles.hint}>{hint}</AppText>
              {notes.length > 0 ? (
                <View style={styles.midContent}>
                  <View style={[styles.notes, { borderColor: theme.colors.border }]}>
                    {notes.map((note) => (
                      <AppText key={note.id} variant="caption" selectable>{note.body}</AppText>
                    ))}
                  </View>
                </View>
              ) : null}
              </>
            }
          />
          {composer ? (
            <View style={[styles.composerSlot, {
              borderColor: theme.colors.border,
              backgroundColor: theme.colors.surface,
            }]}>
              <ScrollView style={styles.composerScroll} keyboardShouldPersistTaps="handled">
                {composer}
              </ScrollView>
            </View>
          ) : null}
          <View style={[styles.footer, { borderColor: theme.colors.border }]}>
            <Button label="Previous" size="sm" variant="ghost" disabled={index <= 0}
              onPress={() => { const prev = visible[index - 1]; if (prev) onSelectPath(prev.path); }} />
            <Button label="Next" size="sm" variant="ghost" disabled={index >= visible.length - 1}
              onPress={() => { const nextFile = visible[index + 1]; if (nextFile) onSelectPath(nextFile.path); }} />
          </View>
        </View>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  empty: { flex: 1, padding: spacing.lg, justifyContent: "center" },
  panes: { flex: 1, minHeight: 0, flexDirection: "row" },
  panesNarrow: { flexDirection: "column" },
  sidebar: { width: 220, borderRightWidth: 1, padding: spacing.sm, gap: spacing.sm, minHeight: 0 },
  sidebarTitle: { textTransform: "uppercase", letterSpacing: 1, paddingHorizontal: 4 },
  sidebarScroll: { flex: 1, minHeight: 0 },
  narrowFiles: { flexGrow: 0, flexShrink: 0, borderBottomWidth: 1, padding: spacing.sm, gap: spacing.sm, maxHeight: 248 },
  narrowFilesHeader: { gap: spacing.xs },
  narrowFilesScroll: { flexGrow: 0, flexShrink: 1, maxHeight: 148 },
  chipRow: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs, paddingVertical: spacing.xs },
  chip: { minHeight: 32, paddingHorizontal: 10, paddingVertical: 4, justifyContent: "center", borderWidth: 1, borderRadius: 10 },
  fileButton: { minHeight: 44, flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: spacing.sm, paddingVertical: spacing.xs, borderRadius: 6 },
  fileTitle: { flex: 1, minWidth: 0 },
  fileCounts: { flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 0 },
  fileCountNum: { fontFamily: Platform.OS === "ios" ? "Menlo" : "monospace", fontWeight: "600" },
  treeRow: { minHeight: 44, flexDirection: "row", alignItems: "center", gap: 6, paddingRight: spacing.sm },
  treeDirLabel: { flex: 1, minWidth: 0 },
  diffPane: { flex: 1, minWidth: 0, minHeight: 0 },
  diffBody: { flex: 1, minHeight: 0 },
  diffRule: { height: 1 },
  fileBar: { paddingHorizontal: spacing.md, paddingVertical: spacing.sm, gap: spacing.sm },
  fileCopy: { gap: 2, minWidth: 0 },
  bold: { fontWeight: "600" },
  commitBanner: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: spacing.sm },
  fileActions: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm },
  splitHeader: { flexDirection: "row", paddingHorizontal: spacing.md, paddingTop: spacing.sm, gap: spacing.sm },
  splitTitle: { flex: 1, minWidth: 0, textTransform: "uppercase", letterSpacing: 1 },
  splitRow: { flexDirection: "row", alignItems: "stretch", paddingVertical: 3, paddingHorizontal: 6, borderRadius: 6, alignSelf: "stretch", gap: 0 },
  splitCell: { flex: 1, minWidth: 0, flexDirection: "row", gap: spacing.sm, alignItems: "flex-start" },
  splitDivider: { width: 1, marginHorizontal: 6, alignSelf: "stretch" },
  hint: { paddingHorizontal: spacing.md, paddingTop: spacing.sm },
  midContent: { paddingBottom: spacing.sm },
  notes: { marginHorizontal: spacing.md, marginTop: spacing.sm, gap: spacing.xs, borderTopWidth: 1, paddingTop: spacing.sm },
  composerSlot: {
    flexGrow: 0, flexShrink: 1,
    marginHorizontal: spacing.md, marginTop: spacing.sm, marginBottom: spacing.sm,
    borderWidth: 1, borderRadius: 14, borderCurve: "continuous",
    maxHeight: 300,
    paddingHorizontal: spacing.md, paddingVertical: spacing.sm,
  },
  composerScroll: { flexGrow: 0 },
  diffScroll: { flex: 1, minHeight: 0 },
  diffContent: { flexGrow: 1, paddingBottom: spacing.lg },
  diffHorizontal: { minWidth: "100%", paddingHorizontal: spacing.md, paddingTop: spacing.sm, flexGrow: 1 },
  diff: { gap: 2, flexShrink: 0, minWidth: "100%", alignSelf: "stretch" },
  diffLine: { flexDirection: "row", gap: spacing.sm, paddingVertical: 3, paddingHorizontal: 6, borderRadius: 6, alignItems: "flex-start", alignSelf: "stretch" },
  linePress: { borderRadius: 6, alignSelf: "stretch" },
  plus: { width: 12, textAlign: "center" },
  lineNumber: { width: 64, textAlign: "right", flexShrink: 0, fontFamily: Platform.OS === "ios" ? "Menlo" : "monospace" },
  code: { fontFamily: Platform.OS === "ios" ? "Menlo" : "monospace", flexShrink: 0, flexGrow: 0 },
  hunkText: { fontWeight: "600" },
  moreRow: { padding: spacing.md, alignItems: "flex-start" },
  footer: { flexDirection: "row", justifyContent: "space-between", gap: spacing.sm, padding: spacing.sm, borderTopWidth: 1 },
});
