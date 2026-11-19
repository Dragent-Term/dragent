import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  FlatList, Platform, StyleSheet, View,
  type ListRenderItemInfo, type NativeScrollEvent, type NativeSyntheticEvent,
} from "react-native";
import { AppText, EmptyState, StatusBadge } from "../components";
import { ScrollFollowing, scrollKeyDirection } from "../components/keyboard";
import { spacing } from "../theme/tokens";
import { useTheme } from "../theme";
import { defaultAgentExpanded, isAtTerminalBottom, terminalItems, type TerminalContent, type TerminalItem } from "./presentation";
import { AgentBlock, AltScreenBlock, ClippedOutput, CommandBlock, rawPendingNote } from "./TerminalItems";
import type { TerminalState } from "./types";

export interface TerminalViewProps { state: TerminalState; content?: TerminalContent; presentation?: "cards" | "canvas" }

const maintainPosition = { minIndexForVisible: 0 };

function itemKey(item: TerminalItem): string { return item.key; }
function ItemSeparator() { return <View style={styles.separator} />; }

function nearBottom(event: NativeSyntheticEvent<NativeScrollEvent>): boolean {
  const { contentOffset, layoutMeasurement, contentSize } = event.nativeEvent;
  return isAtTerminalBottom(contentOffset.y, layoutMeasurement.height, contentSize.height);
}

/**
 * A virtualized styled viewer of host-rendered snapshots. One scrolling
 * lifecycle serves terminal output, agent conversation, or both. Incoming
 * output follows only while the reader is at the bottom; native anchoring
 * preserves position when history trims.
 */
export function TerminalView({ state, content = "all", presentation = "cards" }: TerminalViewProps) {
  // Each filter starts at its own latest output, with a fresh native offset and
  // follow state. No stale scrollback/interaction state crosses mode switches.
  return <TerminalOutput key={`${content}:${presentation}`} state={state} content={content} presentation={presentation} />;
}

function TerminalOutput({ state, content = "all", presentation = "cards" }: TerminalViewProps) {
  const { theme } = useTheme();
  // Expand/collapse lives here — keyed by agent block id — so a reader's
  // choice survives re-renders and incoming deltas. It resets with this
  // component's key on content/presentation switches, like follow state.
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const toggleAgent = useCallback((key: string) => {
    setExpanded((prev) => ({ ...prev, [key]: !(prev[key] ?? false) }));
  }, []);
  const renderItem = useCallback(({ item }: ListRenderItemInfo<TerminalItem>) => {
    switch (item.kind) {
      case "command": return <CommandBlock block={item.block} presentation={presentation} />;
      case "agent": {
        const override = expanded[item.key];
        return (
          <AgentBlock
            block={item.block}
            presentation={presentation}
            expanded={override ?? defaultAgentExpanded(item.block.kind)}
            onToggle={() => toggleAgent(item.key)}
          />
        );
      }
      case "alt": return <AltScreenBlock screen={item.screen} presentation={presentation} />;
    }
  }, [presentation, expanded, toggleAgent]);
  const list = useRef<FlatList<TerminalItem>>(null);
  const scroll = useRef(new ScrollFollowing());
  const data = useMemo(
    () => terminalItems(state, content),
    [state.blocks, state.agentBlocks, state.altScreen, content],
  );

  const followOutput = useCallback(() => {
    if (scroll.current.canFollow) {
      list.current?.scrollToEnd({ animated: false });
    }
  }, []);
  const startScroll = useCallback(() => { scroll.current.beginInteraction(); }, []);
  const onScroll = useCallback((event: NativeSyntheticEvent<NativeScrollEvent>) => {
    scroll.current.scrolled(nearBottom(event));
  }, []);
  const endScroll = useCallback((event: NativeSyntheticEvent<NativeScrollEvent>) => {
    scroll.current.endInteraction(nearBottom(event));
  }, []);

  useEffect(() => {
    if (Platform.OS !== "web") return;
    const node = list.current?.getNativeScrollRef() as unknown as HTMLElement | null;
    if (!node) return;
    // RN Web does not synthesize native drag/momentum events for wheel or
    // keyboard scrolling. Capture intent on the actual scroll viewport instead.
    const wheel = (event: WheelEvent) => {
      if (event.deltaY < 0) scroll.current.towardHistory(node.scrollTop);
    };
    const keyDown = (event: KeyboardEvent) => {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
      if ((event.key === " " || event.key === "Spacebar") && target?.closest('button, a, [role="button"], [role="radio"]')) return;
      if (scrollKeyDirection(event.key, event.shiftKey) === -1) scroll.current.towardHistory(node.scrollTop);
    };
    const pointerDown = (event: PointerEvent) => {
      if (event.button === 0) scroll.current.beginInteraction();
    };
    const pointerEnd = () => {
      if (scroll.current.isInteracting) {
        scroll.current.endInteraction(isAtTerminalBottom(node.scrollTop, node.clientHeight, node.scrollHeight));
      }
    };
    node.addEventListener("wheel", wheel, { capture: true, passive: true });
    node.addEventListener("keydown", keyDown, true);
    node.addEventListener("pointerdown", pointerDown, true);
    // Release outside the viewport must not leave output following suspended.
    document.addEventListener("pointerup", pointerEnd, true);
    document.addEventListener("pointercancel", pointerEnd, true);
    window.addEventListener("blur", pointerEnd);
    return () => {
      node.removeEventListener("wheel", wheel, true);
      node.removeEventListener("keydown", keyDown, true);
      node.removeEventListener("pointerdown", pointerDown, true);
      document.removeEventListener("pointerup", pointerEnd, true);
      document.removeEventListener("pointercancel", pointerEnd, true);
      window.removeEventListener("blur", pointerEnd);
    };
  }, []);

  const showTerminal = content !== "agent";
  const showAgent = content !== "terminal";
  // Command history and agent conversation share one header: working
  // directory/activity for terminal modes, live status for agent modes, then
  // retention and raw-buffer notices. An empty header renders nothing so short
  // histories do not gain stray spacing above the first block.
  const showCwd = presentation !== "canvas" && showTerminal && !!state.surface?.cwd;
  const showActivity = presentation !== "canvas" && showTerminal && !!state.surface?.activity;
  const showAiStatus = presentation !== "canvas" && showAgent && !!state.surface?.ai_status;
  const showClipped = state.historyTruncated;
  const showRawPending = showTerminal && state.rawOutputPending;
  const hasHeader = showCwd || showActivity || showAiStatus || showClipped || showRawPending;

  return (
    <FlatList
      ref={list}
      accessibilityLabel={content === "agent" ? "Agent conversation" : "Terminal output"}
      {...(Platform.OS === "web" ? { tabIndex: 0 as const, role: "region" as const } : {})}
      style={[styles.fill, presentation === "canvas" ? { backgroundColor: content === "terminal" ? theme.terminal.background : theme.colors.background } : null]}
      contentContainerStyle={[styles.listContent, presentation === "canvas" ? styles.canvas : null]}
      data={data}
      renderItem={renderItem}
      keyExtractor={itemKey}
      ItemSeparatorComponent={ItemSeparator}
      initialNumToRender={6}
      maxToRenderPerBatch={4}
      windowSize={7}
      maintainVisibleContentPosition={maintainPosition}
      onContentSizeChange={followOutput}
      onLayout={followOutput}
      onScroll={onScroll}
      onScrollBeginDrag={startScroll}
      onScrollEndDrag={endScroll}
      onMomentumScrollBegin={startScroll}
      onMomentumScrollEnd={endScroll}
      scrollEventThrottle={16}
      keyboardShouldPersistTaps="handled"
      keyboardDismissMode="on-drag"
      contentInsetAdjustmentBehavior="never"
      automaticallyAdjustContentInsets={false}
      automaticallyAdjustKeyboardInsets={false}
      ListHeaderComponent={
        hasHeader ? (
          <View style={presentation === "canvas" ? undefined : styles.summary}>
            {showCwd ? <AppText variant="caption" tone="textMuted" selectable>{state.surface!.cwd}</AppText> : null}
            {showActivity ? <AppText variant="caption" tone="textMuted">{state.surface!.activity}</AppText> : null}
            {showAiStatus ? <StatusBadge label={state.surface!.ai_status!} tone="accent" /> : null}
            {showClipped ? <ClippedOutput /> : null}
            {showRawPending ? <AppText variant="caption" tone="textMuted">{rawPendingNote}</AppText> : null}
          </View>
        ) : undefined
      }
      ListEmptyComponent={
        content === "agent" ? (
          state.surface?.ai_status ? (
            <EmptyState
              title="Agent activity in progress"
              message={`${state.surface.ai_status} · Conversation entries will appear when the host shares them.`}
            />
          ) : (
            <EmptyState
              title="No agent conversation yet"
              message="Agent messages will appear when the host shares them."
            />
          )
        ) : content === "terminal" ? (
          <EmptyState
            title="Waiting for terminal output"
            message={state.rawOutputPending
              ? rawPendingNote
              : "Terminal output will appear when the host shares styled snapshots."}
          />
        ) : (
          <EmptyState
            title="Waiting for output"
            message={state.rawOutputPending
              ? rawPendingNote
              : "Output will appear when the host shares it."}
          />
        )
      }
    />
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1, minHeight: 0 },
  canvas: { padding: 12 },
  listContent: { flexGrow: 1, paddingBottom: spacing.lg },
  summary: { gap: spacing.sm, marginBottom: spacing.md },
  separator: { height: spacing.md },
});
