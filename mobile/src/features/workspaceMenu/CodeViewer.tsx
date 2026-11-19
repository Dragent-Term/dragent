import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ActivityIndicator, FlatList, Image, Platform, ScrollView, StyleSheet, Text, View, type ListRenderItemInfo } from "react-native";
import { AppText, Button, EmptyState } from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import type { RelayRpcClient } from "../../transport/relayRpcClient";
import { openFilePage, readFileImage, type FileImage } from "../../transport/relayRpcMethods";
import { Markdown } from "../agent/AgentMarkdown";
import { codeLanguage, highlightLine, type CodeCarry, type CodeToken, type CodeTokenKind } from "./codeHighlight";
import { sizeLabel } from "./paths";

const MONO = Platform.select({ ios: "Menlo", android: "monospace", default: "monospace" });
const FONT_SIZE = 12;
const LINE_HEIGHT = 18;
/** Menlo and the platform monospace fonts are 0.6 em wide. */
const CHAR_WIDTH = FONT_SIZE * 0.602;
/** Lines longer than this are cut on screen (a minified file would freeze the list). */
const MAX_LINE_CHARS = 2_000;

interface CodeLine {
  readonly number: number;
  readonly tokens: readonly CodeToken[];
  readonly length: number;
}

interface Loaded {
  readonly lines: readonly CodeLine[];
  /** A last line still waiting for the rest of it on the next page. */
  readonly partial: string;
  readonly carry: CodeCarry | null;
  readonly nextOffset: number | null;
  readonly size: number;
}

const EMPTY: Loaded = { lines: [], partial: "", carry: null, nextOffset: 0, size: 0 };

/** Add one page of text to what is loaded, colouring only the new lines. */
function appendPage(previous: Loaded, content: string, nextOffset: number | null, size: number, language: string): Loaded {
  const pieces = (previous.partial + content).split("\n");
  // Until the last page, the final piece may be half a line.
  const partial = nextOffset === null ? "" : (pieces.pop() ?? "");
  if (nextOffset === null && pieces.length > 1 && pieces[pieces.length - 1] === "") pieces.pop();
  const lines = [...previous.lines];
  let carry = previous.carry;
  for (const raw of pieces) {
    const text = raw.length > MAX_LINE_CHARS ? `${raw.slice(0, MAX_LINE_CHARS)} …` : raw.replace(/\r$/u, "");
    const result = highlightLine(text.replace(/\t/gu, "  "), language, carry);
    carry = result.carry;
    lines.push({ number: lines.length + 1, tokens: result.tokens, length: text.length });
  }
  return { lines, partial, carry, nextOffset, size };
}

function useTokenColors(): Record<CodeTokenKind, string> {
  const { theme } = useTheme();
  return useMemo(() => {
    const ansi = theme.terminal.ansiNormal;
    return {
      plain: theme.colors.text,
      comment: theme.colors.textSubtle,
      string: ansi[2] ?? theme.colors.success,
      number: ansi[3] ?? theme.colors.warning,
      keyword: ansi[5] ?? theme.colors.accent,
      type: ansi[6] ?? theme.colors.accent,
    };
  }, [theme]);
}

const Line = memo(function Line({ line, gutter, colors, wrap }: {
  line: CodeLine;
  gutter: number;
  colors: Record<CodeTokenKind, string>;
  wrap: boolean;
}) {
  const { theme } = useTheme();
  return (
    <View style={styles.line}>
      <Text style={[styles.code, styles.number, { width: gutter, color: theme.colors.textSubtle }]}>{line.number}</Text>
      <Text selectable={wrap} style={[styles.code, wrap ? styles.wrapText : null]}>
        {line.tokens.map((token, index) => (
          <Text key={index} style={{ color: colors[token.kind] }}>{token.text}</Text>
        ))}
      </Text>
    </View>
  );
});

/**
 * Read-only code view of one file on the desktop: line numbers, syntax
 * colours, sideways scrolling for long lines (or wrapping), and more of the
 * file loaded as the reader nears the end.
 */
export function CodeViewer({ rpc, path }: { rpc: RelayRpcClient; path: string }) {
  if (IMAGE_FILE.test(path)) return <ImageViewer rpc={rpc} path={path} />;
  return <SourceViewer rpc={rpc} path={path} />;
}

const IMAGE_FILE = /\.(png|jpe?g|gif|webp)$/i;
const MARKDOWN_FILE = /\.(md|markdown|mdx)$/i;

/** An image on the desktop, fitted to the screen; pinch to zoom. */
function ImageViewer({ rpc, path }: { rpc: RelayRpcClient; path: string }) {
  const { theme } = useTheme();
  const [image, setImage] = useState<FileImage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    setImage(null);
    setError(null);
    readFileImage(rpc, path)
      .then((next) => { if (active) setImage(next); })
      .catch(() => { if (active) setError("This image couldn't be loaded from your desktop."); });
    return () => { active = false; };
  }, [rpc, path, attempt]);
  if (error) return <EmptyState title="Can't show this image" message={error} actionLabel="Try again" onAction={() => setAttempt((n) => n + 1)} />;
  if (!image) return <View style={styles.center}><ActivityIndicator color={theme.colors.accent} /></View>;
  if (image.tooLarge || !image.data) {
    return <EmptyState title="Image too large to preview" message={`It is ${sizeLabel(image.size)}. Open it on your desktop.`} />;
  }
  return (
    <View style={[styles.flex, { backgroundColor: theme.colors.background }]}>
      <View style={[styles.bar, { borderColor: theme.colors.border }]}>
        <AppText variant="caption" tone="textMuted" style={styles.flex} numberOfLines={1}>
          {image.mime.replace("image/", "").toUpperCase()} · {sizeLabel(image.size)} · pinch to zoom
        </AppText>
      </View>
      <ScrollView
        style={styles.flex}
        contentContainerStyle={styles.imageFrame}
        maximumZoomScale={5}
        minimumZoomScale={1}
        centerContent
      >
        <Image
          source={{ uri: `data:${image.mime};base64,${image.data}` }}
          style={styles.image}
          resizeMode="contain"
          accessibilityLabel={`Image ${path.split("/").pop() ?? ""}`}
        />
      </ScrollView>
    </View>
  );
}

function SourceViewer({ rpc, path }: { rpc: RelayRpcClient; path: string }) {
  const { theme } = useTheme();
  const markdown = MARKDOWN_FILE.test(path);
  const [preview, setPreview] = useState(markdown);
  const colors = useTokenColors();
  const language = useMemo(() => codeLanguage(path), [path]);
  const [loaded, setLoaded] = useState<Loaded>(EMPTY);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [wrap, setWrap] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const busy = useRef(false);
  const state = useRef(loaded);
  state.current = loaded;

  const loadMore = useCallback(() => {
    const offset = state.current.nextOffset;
    if (busy.current || offset === null) return;
    busy.current = true;
    setLoading(true);
    setError(null);
    openFilePage(rpc, path, offset)
      .then((page) => {
        setLoaded((previous) => appendPage(previous, page.content, page.nextOffset, page.size, language));
      })
      .catch((failure: unknown) => {
        const message = (failure as { message?: unknown } | null)?.message;
        setError(
          offset === 0
            ? "This file can't be shown here. It may be binary, too large, or no longer there."
            : typeof message === "string" && message !== "" ? message : "The rest of the file couldn't be loaded.",
        );
      })
      .finally(() => {
        busy.current = false;
        setLoading(false);
      });
  }, [rpc, path, language]);

  useEffect(() => {
    busy.current = false;
    setLoaded(EMPTY);
    state.current = EMPTY;
    loadMore();
  }, [path, attempt, loadMore]);

  const gutter = Math.max(3, String(loaded.lines.length).length) * CHAR_WIDTH + 14;
  const widest = useMemo(() => loaded.lines.reduce((max, line) => Math.max(max, line.length), 0), [loaded.lines]);
  const contentWidth = gutter + widest * CHAR_WIDTH + spacing.lg * 2;
  const renderItem = useCallback(
    ({ item }: ListRenderItemInfo<CodeLine>) => <Line line={item} gutter={gutter} colors={colors} wrap={wrap} />,
    [gutter, colors, wrap],
  );
  const getItemLayout = useCallback(
    (_: unknown, index: number) => ({ length: LINE_HEIGHT, offset: LINE_HEIGHT * index, index }),
    [],
  );

  if (error && loaded.lines.length === 0) {
    return (
      <EmptyState title="Can't open this file" message={error} actionLabel="Try again" onAction={() => setAttempt((n) => n + 1)} />
    );
  }
  if (loaded.lines.length === 0) {
    return (
      <View style={styles.center}>
        {loading ? <ActivityIndicator color={theme.colors.accent} /> : <AppText tone="textMuted">This file is empty.</AppText>}
      </View>
    );
  }

  const list = (
    <FlatList
      data={loaded.lines}
      keyExtractor={(line) => String(line.number)}
      renderItem={renderItem}
      {...(wrap ? {} : { getItemLayout })}
      onEndReached={loadMore}
      onEndReachedThreshold={1.5}
      initialNumToRender={60}
      maxToRenderPerBatch={80}
      windowSize={11}
      style={wrap ? styles.flex : { width: Math.max(contentWidth, 320) }}
      contentContainerStyle={styles.content}
      ListFooterComponent={
        loaded.nextOffset !== null ? (
          <View style={styles.footer}>
            {error ? <Button label="Load the rest" size="sm" variant="ghost" onPress={loadMore} /> : <ActivityIndicator color={theme.colors.textMuted} />}
          </View>
        ) : undefined
      }
    />
  );

  return (
    <View style={[styles.flex, { backgroundColor: theme.colors.background }]}>
      <View style={[styles.bar, { borderColor: theme.colors.border }]}>
        <AppText variant="caption" tone="textMuted" style={styles.flex} numberOfLines={1}>
          {loaded.lines.length}{loaded.nextOffset !== null ? "+" : ""} lines · {sizeLabel(loaded.size)} · read only
        </AppText>
        {markdown ? (
          <Button label={preview ? "Source" : "Preview"} size="sm" variant="ghost" onPress={() => setPreview((value) => !value)} />
        ) : null}
        {!preview ? <Button label={wrap ? "Wrap: on" : "Wrap: off"} size="sm" variant="ghost" onPress={() => setWrap((value) => !value)} /> : null}
      </View>
      {preview ? (
        <ScrollView style={styles.flex} contentContainerStyle={styles.markdown}>
          <Markdown text={loaded.lines.map((line) => line.tokens.map((token) => token.text).join("")).join("\n")} />
          {loaded.nextOffset !== null ? (
            <Button label="Load the rest" size="sm" variant="ghost" onPress={loadMore} />
          ) : null}
        </ScrollView>
      ) : wrap ? list : (
        <ScrollView horizontal style={styles.flex} showsHorizontalScrollIndicator contentContainerStyle={styles.grow}>
          {list}
        </ScrollView>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  imageFrame: { flexGrow: 1, alignItems: "center", justifyContent: "center", padding: spacing.md },
  image: { width: "100%", height: undefined, aspectRatio: 1, minHeight: 240 },
  markdown: { padding: spacing.lg, paddingBottom: spacing.xl, gap: spacing.sm },
  flex: { flex: 1, minWidth: 0, minHeight: 0 },
  grow: { flexGrow: 1 },
  center: { flex: 1, alignItems: "center", justifyContent: "center", padding: spacing.xl },
  bar: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    paddingLeft: spacing.md,
    paddingRight: spacing.xs,
    borderBottomWidth: StyleSheet.hairlineWidth,
    minHeight: 36,
  },
  content: { paddingVertical: spacing.sm, paddingRight: spacing.lg },
  line: { flexDirection: "row", minHeight: LINE_HEIGHT },
  code: { fontFamily: MONO, fontSize: FONT_SIZE, lineHeight: LINE_HEIGHT },
  number: { textAlign: "right", paddingRight: 10 },
  wrapText: { flex: 1, minWidth: 0 },
  footer: { paddingVertical: spacing.md, alignItems: "flex-start", paddingLeft: spacing.lg },
});
