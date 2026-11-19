import { useCallback, useEffect, useRef, useState, type ComponentRef, type ReactNode } from "react";
import { Platform, ScrollView, StyleSheet, View } from "react-native";
import { useTheme } from "../theme";
import { componentPaddingScale } from "../theme/tokens";
import { hiddenAttention, type StripLayout } from "../machines/workspaceTabs";
import { AgentStateDot } from "./AgentStateDot";
import { AppText } from "./AppText";
import { Control } from "./Control";
import type { SegmentedControlProps, SegmentedOption } from "./SegmentedControl";
import { useChoiceKeyboard } from "./useChoiceKeyboard";

export interface TabStripProps<T extends string> extends SegmentedControlProps<T> {
  options: readonly (SegmentedOption<T> & { leading?: ReactNode })[];
  /** Kept in place at the strip's end however far it scrolls (a "+", say). */
  trailing?: ReactNode;
  /**
   * Options waiting for the person. One scrolled out of sight shows a dot at
   * that edge of the strip, so it isn't missed among many.
   */
  attention?: ReadonlySet<T>;
  /** Long-press on an option (its actions). */
  onLongPress?: (id: T) => void;
}

/** Open desktop tabs and scoped project views share this quiet horizontal strip. */
export function TabStrip<T extends string>({ label, options, value, onChange, trailing, attention, onLongPress }: TabStripProps<T>) {
  const { theme } = useTheme();
  const group = useRef<ComponentRef<typeof View>>(null);
  const scroll = useRef<ComponentRef<typeof ScrollView>>(null);
  const layouts = useRef(new Map<string, StripLayout>());
  const offset = useRef(0);
  const viewport = useRef(0);
  const [edges, setEdges] = useState({ left: false, right: false });
  const activate = useCallback((index: number) => {
    const option = options[index];
    if (option) onChange(option.id);
  }, [options, onChange]);
  useChoiceKeyboard(group, "tab", options.length, activate);
  // Only re-render when an edge's dot appears or goes, not on every scroll frame.
  const measureEdges = useCallback(() => {
    if (!attention || attention.size === 0) {
      setEdges((prev) => (prev.left || prev.right ? { left: false, right: false } : prev));
      return;
    }
    const next = hiddenAttention(layouts.current, attention as ReadonlySet<string>, offset.current, viewport.current);
    setEdges((prev) => (prev.left === next.left && prev.right === next.right ? prev : next));
  }, [attention]);
  useEffect(measureEdges, [measureEdges, options]);
  useEffect(() => {
    const at = layouts.current.get(value);
    if (at !== undefined) scroll.current?.scrollTo({ x: Math.max(0, at.x - 12), animated: false });
  }, [value]);
  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
      <View style={styles.scrollArea}>
        <ScrollView ref={scroll} horizontal showsHorizontalScrollIndicator={false}
          keyboardShouldPersistTaps="handled" style={styles.scroll}
          scrollEventThrottle={32}
          onLayout={(event) => {
            viewport.current = event.nativeEvent.layout.width;
            measureEdges();
          }}
          onScroll={(event) => {
            offset.current = event.nativeEvent.contentOffset.x;
            measureEdges();
          }}>
          <View ref={group} accessibilityRole="tablist" accessibilityLabel={label} style={styles.group}>
            {options.map((option) => {
              const selected = option.id === value;
              return (
                <Control key={option.id} accessibilityRole="tab" accessibilityLabel={option.hint ?? option.label}
                  accessibilityState={{ selected }}
                  {...(Platform.OS === "web" ? { tabIndex: selected ? 0 as const : -1 as const } : {})}
                  onPress={() => onChange(option.id)}
                  onLongPress={onLongPress ? () => onLongPress(option.id) : undefined}
                  onLayout={(event) => {
                    const { x, width } = event.nativeEvent.layout;
                    layouts.current.set(option.id, { x, width });
                    if (selected) scroll.current?.scrollTo({ x: Math.max(0, x - 12), animated: false });
                    measureEdges();
                  }}
                  style={({ pressed, hovered }) => [styles.tab, { borderColor: selected ? theme.colors.accent : theme.colors.surface,
                    backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.surface }]}>
                  {option.leading}
                  <AppText variant="caption" tone={selected ? "text" : "textMuted"} numberOfLines={1} style={styles.tabLabel}>{option.label}</AppText>
                </Control>
              );
            })}
          </View>
        </ScrollView>
        {edges.left ? (
          <View pointerEvents="none" style={[styles.edge, styles.edgeLeft, { backgroundColor: theme.colors.surface }]}
            accessibilityLabel="More tabs that need you to the left">
            <AgentStateDot state="waiting" />
          </View>
        ) : null}
        {edges.right ? (
          <View pointerEvents="none" style={[styles.edge, styles.edgeRight, { backgroundColor: theme.colors.surface }]}
            accessibilityLabel="More tabs that need you to the right">
            <AgentStateDot state="waiting" />
          </View>
        ) : null}
      </View>
      {trailing ? <View style={[styles.trailing, { borderColor: theme.colors.border }]}>{trailing}</View> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flexGrow: 0, flexShrink: 0, borderBottomWidth: 1, flexDirection: "row", alignItems: "stretch" },
  scrollArea: { flex: 1, minWidth: 0 },
  scroll: { flexGrow: 0 },
  group: { flexDirection: "row", alignItems: "stretch" },
  tab: { minHeight: 44, minWidth: 76, maxWidth: 260, paddingHorizontal: 14 * componentPaddingScale, flexDirection: "row", alignItems: "center", gap: 6, justifyContent: "center", borderBottomWidth: 2 },
  tabLabel: { flexShrink: 1, minWidth: 0 },
  edge: { position: "absolute", top: 0, bottom: 2, width: 22, alignItems: "center", justifyContent: "center", opacity: 0.96 },
  edgeLeft: { left: 0 },
  edgeRight: { right: 0 },
  trailing: { flexDirection: "row", alignItems: "center", gap: 2, paddingHorizontal: 4, borderLeftWidth: 1 },
});
