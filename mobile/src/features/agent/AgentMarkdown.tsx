import { memo, useMemo } from "react";
import { Linking, Platform, ScrollView, StyleSheet, Text, View } from "react-native";
import { useTheme } from "../../theme";
import { radii, spacing, type } from "../../theme/tokens";
import { markdownBlocks, type MarkdownInline } from "./markdownText";

const MONO = Platform.select({ ios: "Menlo", android: "monospace", default: "monospace" });

function Inline({ runs, color, codeBackground, accent }: {
  runs: readonly MarkdownInline[];
  color: string;
  codeBackground: string;
  accent: string;
}) {
  return (
    <>
      {runs.map((run, index) => {
        switch (run.kind) {
          case "bold": return <Text key={index} style={styles.bold}>{run.text}</Text>;
          case "italic": return <Text key={index} style={styles.italic}>{run.text}</Text>;
          case "code":
            return <Text key={index} style={[styles.inlineCode, { backgroundColor: codeBackground, color }]}>{run.text}</Text>;
          case "link":
            return (
              <Text
                key={index}
                accessibilityRole="link"
                style={{ color: accent }}
                onPress={() => { void Linking.openURL(run.href).catch(() => {}); }}
              >
                {run.text}
              </Text>
            );
          default: return <Text key={index}>{run.text}</Text>;
        }
      })}
    </>
  );
}

/**
 * The agent's Markdown, drawn with the theme. `muted` is for quieter text
 * (thinking, notes); `reading` is the larger size for a chat reply.
 */
export const Markdown = memo(function Markdown({ text, muted = false, reading = false }: {
  text: string;
  muted?: boolean;
  reading?: boolean;
}) {
  const { theme } = useTheme();
  const blocks = useMemo(() => markdownBlocks(text), [text]);
  const color = muted ? theme.colors.textMuted : theme.colors.text;
  const body = reading ? styles.reading : styles.body;
  const inline = { color, codeBackground: theme.colors.surfaceRaised, accent: theme.colors.accent };
  return (
    <View style={styles.column}>
      {blocks.map((block, index) => {
        switch (block.kind) {
          case "heading":
            return (
              <Text key={index} selectable style={[body, block.level <= 2 ? (reading ? styles.readingHeading : styles.headingLarge) : styles.heading, { color }]}>
                <Inline runs={block.inline} {...inline} />
              </Text>
            );
          case "item":
            return (
              <View key={index} style={[styles.item, { paddingLeft: block.depth * 14 }]}>
                <Text style={[body, styles.marker, { color: theme.colors.textMuted }]}>{block.marker}</Text>
                <Text selectable style={[body, styles.itemText, { color }]}>
                  <Inline runs={block.inline} {...inline} />
                </Text>
              </View>
            );
          case "quote":
            return (
              <View key={index} style={[styles.quote, { borderColor: theme.colors.border }]}>
                <Text selectable style={[body, { color: theme.colors.textMuted }]}>
                  <Inline runs={block.inline} {...inline} />
                </Text>
              </View>
            );
          case "code":
            return (
              <View key={index} style={[styles.code, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
                <ScrollView horizontal showsHorizontalScrollIndicator={false}>
                  <Text selectable style={[styles.codeText, { color: theme.colors.text }]}>{block.text}</Text>
                </ScrollView>
              </View>
            );
          case "rule":
            return <View key={index} style={[styles.rule, { backgroundColor: theme.colors.border }]} />;
          default:
            return (
              <Text key={index} selectable style={[body, { color }]}>
                <Inline runs={block.inline} {...inline} />
              </Text>
            );
        }
      })}
    </View>
  );
});

const styles = StyleSheet.create({
  column: { gap: spacing.sm, minWidth: 0 },
  body: { fontSize: type.body.fontSize, lineHeight: type.body.lineHeight },
  reading: { fontSize: 15, lineHeight: 23 },
  readingHeading: { fontWeight: "600", fontSize: 17, lineHeight: 25 },
  bold: { fontWeight: "600" },
  italic: { fontStyle: "italic" },
  inlineCode: { fontFamily: MONO, fontSize: 12.5 },
  heading: { fontWeight: "600" },
  headingLarge: { fontWeight: "600", fontSize: 16, lineHeight: 23 },
  item: { flexDirection: "row", gap: 6 },
  marker: { minWidth: 14 },
  itemText: { flex: 1, minWidth: 0 },
  quote: { borderLeftWidth: 2, paddingLeft: spacing.sm },
  code: { borderRadius: radii.md, borderWidth: StyleSheet.hairlineWidth, paddingHorizontal: 10, paddingVertical: 8 },
  codeText: { fontFamily: MONO, fontSize: 12, lineHeight: 17 },
  rule: { height: StyleSheet.hairlineWidth },
});
