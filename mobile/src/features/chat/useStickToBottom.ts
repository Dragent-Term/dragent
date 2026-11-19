import { useCallback, useRef, useState, type RefObject } from "react";
import type { FlatList, LayoutChangeEvent, NativeScrollEvent, NativeSyntheticEvent } from "react-native";

/** Within this many points of the end counts as "at the latest". */
const AT_END_PX = 12;

function atEnd(event: NativeSyntheticEvent<NativeScrollEvent>): boolean {
  const { contentOffset, contentSize, layoutMeasurement } = event.nativeEvent;
  return contentSize.height - layoutMeasurement.height - contentOffset.y < AT_END_PX;
}

/**
 * Keeps a conversation pinned to its newest line while it streams. Only the
 * person's own finger can unpin it: content growing (a reply streaming in, a
 * step arriving), the keyboard opening or the message box growing never
 * count as "scrolled away". Scrolling back to the end pins it again.
 */
export function useStickToBottom<T>() {
  const list = useRef<FlatList<T>>(null) as RefObject<FlatList<T> | null>;
  const following = useRef(true);
  const touching = useRef(false);
  const gliding = useRef(false);
  const [away, setAway] = useState(false);

  const pin = useCallback(() => {
    if (!following.current) return;
    list.current?.scrollToEnd({ animated: false });
    // Again once the new lines have their real height: a first jump can
    // stop short of an end that is still being measured.
    requestAnimationFrame(() => {
      if (following.current) list.current?.scrollToEnd({ animated: false });
    });
  }, []);
  /** Where a drag started: scrolling up by hand always stops following. */
  const dragFrom = useRef(0);
  const settle = useCallback((event: NativeSyntheticEvent<NativeScrollEvent>) => {
    const movedUp = event.nativeEvent.contentOffset.y < dragFrom.current - 2;
    following.current = atEnd(event) && !movedUp;
    if (following.current !== !away) setAway(!following.current);
  }, [away]);

  const listProps = {
    ref: list,
    scrollEventThrottle: 32,
    // Every line stays drawn at its real height. A conversation is bounded
    // (older lines live on the desktop), and lines drawn only near the
    // screen, at guessed heights, both moved what you were reading when
    // they were measured and made "go to the end" stop short.
    initialNumToRender: 60,
    maxToRenderPerBatch: 40,
    windowSize: 201,
    removeClippedSubviews: false,
    // While you read back, lines changing above you don't move what you see.
    maintainVisibleContentPosition: { minIndexForVisible: 0 },
    onScrollBeginDrag: (event: NativeSyntheticEvent<NativeScrollEvent>) => {
      touching.current = true;
      dragFrom.current = event.nativeEvent.contentOffset.y;
    },
    onScrollEndDrag: (event: NativeSyntheticEvent<NativeScrollEvent>) => {
      touching.current = false;
      if (!gliding.current) settle(event);
    },
    onMomentumScrollBegin: () => { gliding.current = true; },
    onMomentumScrollEnd: (event: NativeSyntheticEvent<NativeScrollEvent>) => {
      gliding.current = false;
      // Where the glide ends decides: at the very end, follow again.
      dragFrom.current = event.nativeEvent.contentOffset.y;
      settle(event);
    },
    onScroll: (event: NativeSyntheticEvent<NativeScrollEvent>) => {
      if (touching.current || gliding.current) settle(event);
    },
    onContentSizeChange: () => pin(),
    onLayout: (_event: LayoutChangeEvent) => pin(),
  };

  /** Back to the newest line, and keep following. */
  const jumpToEnd = useCallback(() => {
    following.current = true;
    setAway(false);
    list.current?.scrollToEnd({ animated: true });
  }, []);
  /** Follow again from the next change (after sending). */
  const follow = useCallback(() => {
    following.current = true;
    setAway(false);
  }, []);

  return { listProps, away, jumpToEnd, follow, pin };
}
