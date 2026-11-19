/**
 * Keeps typing in view. `KeyboardFrame` lifts its bottom edge (and anything
 * docked there, like a message box) above the keyboard by exactly the part
 * the keyboard covers, measured against the frame itself, so it is right
 * whatever sits above or below the screen. `KeyboardScrollView` scrolls the
 * focused field back into view when the keyboard opens, the view shrinks, or
 * the field grows while typing.
 */
import { useCallback, useEffect, useRef, useState, type ComponentRef, type ReactNode } from "react";
import {
  Keyboard,
  Platform,
  ScrollView,
  TextInput,
  View,
  type KeyboardEvent,
  type ScrollViewProps,
  type StyleProp,
  type ViewStyle,
} from "react-native";

/** Space kept between the focused field and the keyboard. */
const MARGIN = 24;

export function KeyboardFrame({ style, children }: { style?: StyleProp<ViewStyle>; children: ReactNode }) {
  const frame = useRef<ComponentRef<typeof View>>(null);
  const [inset, setInset] = useState(0);
  useEffect(() => {
    const show = (event: KeyboardEvent) => {
      const top = event.endCoordinates.screenY;
      frame.current?.measureInWindow((_x, y, _width, height) => {
        setInset(Math.max(0, Math.round(y + height - top)));
      });
    };
    const hide = () => setInset(0);
    const subscriptions =
      Platform.OS === "ios"
        ? [Keyboard.addListener("keyboardWillChangeFrame", show), Keyboard.addListener("keyboardWillHide", hide)]
        : [Keyboard.addListener("keyboardDidShow", show), Keyboard.addListener("keyboardDidHide", hide)];
    return () => subscriptions.forEach((subscription) => subscription.remove());
  }, []);
  return (
    <View ref={frame} style={[{ flex: 1, minHeight: 0 }, style, { paddingBottom: inset }]}>
      {children}
    </View>
  );
}

export function KeyboardScrollView({ children, onLayout, onScroll, onContentSizeChange, ...props }: ScrollViewProps) {
  const scroll = useRef<ComponentRef<typeof ScrollView>>(null);
  const offset = useRef(0);
  const height = useRef(0);

  const reveal = useCallback(() => {
    const field = TextInput.State.currentlyFocusedInput();
    const inner = scroll.current?.getInnerViewRef();
    if (!field || !inner || height.current <= 0) return;
    // Fails quietly when the focused field isn't inside this scroll view.
    field.measureLayout(
      inner,
      (_x, y, _width, fieldHeight) => {
        const top = offset.current;
        const bottom = top + height.current;
        if (y - MARGIN < top) {
          scroll.current?.scrollTo({ y: Math.max(0, y - MARGIN), animated: true });
        } else if (y + fieldHeight + MARGIN > bottom) {
          const target = fieldHeight + MARGIN * 2 > height.current ? y - MARGIN : y + fieldHeight + MARGIN - height.current;
          scroll.current?.scrollTo({ y: Math.max(0, target), animated: true });
        }
      },
      () => {},
    );
  }, []);

  useEffect(() => {
    // The frame shrinking calls onLayout too; this covers a keyboard that
    // opens without changing this view's size.
    const subscription = Keyboard.addListener("keyboardDidShow", () => reveal());
    return () => subscription.remove();
  }, [reveal]);

  return (
    <ScrollView
      ref={scroll}
      keyboardShouldPersistTaps="handled"
      keyboardDismissMode="interactive"
      automaticallyAdjustKeyboardInsets={false}
      scrollEventThrottle={32}
      {...props}
      onLayout={(event) => {
        height.current = event.nativeEvent.layout.height;
        reveal();
        onLayout?.(event);
      }}
      onScroll={(event) => {
        offset.current = event.nativeEvent.contentOffset.y;
        onScroll?.(event);
      }}
      onContentSizeChange={(width, contentHeight) => {
        // A growing field pushes its own bottom down: follow it.
        reveal();
        onContentSizeChange?.(width, contentHeight);
      }}
    >
      {children}
    </ScrollView>
  );
}
