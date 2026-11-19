import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  FlatList, Platform, StyleSheet, View,
  type ListRenderItemInfo, type NativeScrollEvent, type NativeSyntheticEvent,
} from "react-native";
import { Button, EmptyState, StatusBadge } from "../../components";
import { ScrollFollowing, scrollKeyDirection } from "../../components/keyboard";
import { spacing } from "../../theme/tokens";
import type { TerminalAgentBlock } from "../../terminal";
import { isAtTerminalBottom } from "../../terminal/presentation";
import { ChatMessage } from "./ChatMessage";
import { ChatClippedNote } from "./ChatDisclosure";
import type { AgentConversationProps } from "./types";

function chatKey(sessionId: string | undefined, block: TerminalAgentBlock): string {
  return `chat:${sessionId ?? "local"}:${block.id}`;
}

function nearBottom(event: NativeSyntheticEvent<NativeScrollEvent>): boolean {
  const { contentOffset, layoutMeasurement, contentSize } = event.nativeEvent;
  return isAtTerminalBottom(contentOffset.y, layoutMeasurement.height, contentSize.height);
}

function sortedBlocks(blocks: readonly TerminalAgentBlock[]): TerminalAgentBlock[] {
  return [...blocks].sort((a, b) => a.tsMs - b.tsMs || a.id - b.id);
}

function renderRow({ item }: ListRenderItemInfo<TerminalAgentBlock>) {
  return <ChatMessage block={item} />;
}

/**
 * Original native agent conversation. Virtualized, read-only, and composer-free:
 * sending, stopping, and input stay in `SessionComposer`. Follows new output
 * only while the reader is at the bottom; otherwise offers jump-to-latest.
 */
export function AgentConversation({ state, blocks, sessionId }: AgentConversationProps) {
  const source = blocks ?? state?.agentBlocks ?? [];
  const data = useMemo(() => sortedBlocks(source), [source]);
  const aiStatus = state?.surface?.ai_status ?? null;
  const truncated = state?.historyTruncated ?? false;
  const list = useRef<FlatList<TerminalAgentBlock>>(null);
  const scroll = useRef(new ScrollFollowing());
  const [away, setAway] = useState(false);
  const [pending, setPending] = useState(false);

  const followOutput = useCallback(() => {
    if (scroll.current.canFollow) {
      list.current?.scrollToEnd({ animated: false });
    } else {
      setPending(true);
    }
  }, []);

  const startScroll = useCallback(() => { scroll.current.beginInteraction(); }, []);
  const onScroll = useCallback((event: NativeSyntheticEvent<NativeScrollEvent>) => {
    const bottom = nearBottom(event);
    scroll.current.scrolled(bottom);
    setAway(!bottom);
    if (bottom) setPending(false);
  }, []);
  const endScroll = useCallback((event: NativeSyntheticEvent<NativeScrollEvent>) => {
    const bottom = nearBottom(event);
    scroll.current.endInteraction(bottom);
    setAway(!bottom);
    if (bottom) setPending(false);
  }, []);
  const jumpToLatest = useCallback(() => {
    scroll.current.endInteraction(true);
    setAway(false);
    setPending(false);
    list.current?.scrollToEnd({ animated: true });
  }, []);

  useEffect(() => {
    if (Platform.OS !== "web") return;
    const node = list.current?.getNativeScrollRef() as unknown as HTMLElement | undefined;
    if (!node || typeof node.addEventListener !== "function") return;
    const wheel = (event: WheelEvent) => {
      if (event.deltaY < 0) scroll.current.towardHistory(node.scrollTop);
    };
    const keyDown = (event: KeyboardEvent) => {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      const target = event.target instanceof Element ? event.target : null;
      // Never steal keys from real inputs or buttons.
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

  const showJump = away && pending && data.length > 0;

  return (
    <View style={styles.fill}>
      <FlatList
        // Reset follow/offset state when the conversation identity changes.
        key={sessionId ?? "chat"}
        ref={list}
        accessibilityLabel="Agent conversation"
        {...(Platform.OS === "web" ? { tabIndex: 0 as const, role: "region" as const } : {})}
        style={styles.fill}
        contentContainerStyle={styles.content}
        data={data}
        renderItem={renderRow}
        keyExtractor={(item) => chatKey(sessionId, item)}
        ItemSeparatorComponent={Separator}
        initialNumToRender={8}
        maxToRenderPerBatch={6}
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
          aiStatus || truncated ? (
            <View style={styles.header}>
              {aiStatus ? <StatusBadge label={aiStatus} tone="accent" /> : null}
              {truncated ? <ChatClippedNote /> : null}
            </View>
          ) : undefined
        }
        ListEmptyComponent={
          aiStatus ? (
            <EmptyState
              title="Agent activity in progress"
              message={`${aiStatus} · Conversation entries will appear when the host shares them.`}
            />
          ) : (
            <EmptyState
              title="No agent conversation yet"
              message="Agent messages will appear when the host shares them."
            />
          )
        }
      />
      {showJump ? (
        <View style={styles.jump}>
          <Button label="Jump to latest" size="sm" variant="ghost" onPress={jumpToLatest} />
        </View>
      ) : null}
    </View>
  );
}

function Separator() {
  return <View style={styles.separator} />;
}

const maintainPosition = { minIndexForVisible: 0 };

const styles = StyleSheet.create({
  fill: { flex: 1, minHeight: 0 },
  content: { flexGrow: 1, paddingHorizontal: spacing.md, paddingTop: spacing.sm, paddingBottom: spacing.lg },
  header: { gap: spacing.sm, marginBottom: spacing.md },
  separator: { height: spacing.md },
  jump: { position: "absolute", left: 0, right: 0, bottom: spacing.md, alignItems: "center" },
});
