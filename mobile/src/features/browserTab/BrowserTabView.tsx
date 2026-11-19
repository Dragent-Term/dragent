import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ActivityIndicator, Animated, AppState, Image, Keyboard, PanResponder, ScrollView, StyleSheet, View, type LayoutChangeEvent } from "react-native";
import { useFocusEffect } from "expo-router";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { AppText, Button, IconButton } from "../../components";
import { FieldInput } from "../../components/FieldInput";
import { useMachine } from "../../machines/MachineContext";
import { useSoftKeyboard } from "../../terminal/companion/useSoftKeyboard";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { DESKTOP_IN_CONTROL_CODE } from "../../transport/relayRpcClient";
import { browserAct, type BrowserAction, type BrowserKey, type BrowserOverlay } from "../../transport/relayRpcMethods";
import { addTile, clampTop, covers, maxTop, nextPeek, windowScrolls, type PageTile } from "./pageTiles";

/** How often the visible screen is looked at again while nothing is being done. */
const REFRESH_MS = 1_200;
/** How often the view checks whether to refresh or to fetch the screen ahead. */
const TICK_MS = 120;
/** A touch that moves less than this is a tap, not a scroll. */
const TAP_SLOP_PX = 8;

type Extra = Parameters<typeof browserAct>[3];

/** The page's own measurements, in its pixels. */
interface PageSize {
  readonly width: number;
  /** Height of the screen the desktop shows. */
  readonly height: number;
  /** Height of the whole page. */
  readonly full: number;
}

function messageOf(failure: unknown): string {
  const text = (failure as { message?: unknown } | null)?.message;
  return typeof text === "string" && text !== "" ? text : "The desktop didn't answer. Try again.";
}

/** Actions after which the page may be a different page: what was seen before is forgotten. */
const CHANGES_PAGE: ReadonlySet<BrowserAction> = new Set(["open", "back", "forward", "reload", "tap", "type", "press"]);

/**
 * A browser tab on the desktop, used from the phone: its address bar, back,
 * forward and reload, the page itself, and a field to type into it.
 * Everything happens in the real tab on the desktop, which shows it as in
 * use from the phone.
 *
 * **Scrolling is local.** The phone keeps the pictures it has been sent,
 * each at its place on the page, and moves across them at once under the
 * finger, like a real page. The page around the reader is fetched ahead, a
 * screen at a time, so scrolling finds it already there; over page that is
 * already known nothing is asked of the desktop until the reader stops.
 * Elements the page keeps in place (a sticky header) arrive apart from the
 * page's pictures and stay pinned here too, as in a real browser. A page that scrolls a panel inside itself (not the window)
 * can't be remembered that way: there each drag scrolls the panel under the
 * finger and the picture updates in place.
 *
 * **Mobile view** lays the page out at this phone's own size; **Desktop
 * view** shows it at the desktop pane's size.
 * `onTakenByDesktop` runs when the desktop takes the tab back.
 */
export function BrowserTabView({ tabId, onTakenByDesktop }: { tabId: number; onTakenByDesktop?: () => void }) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const keyboard = useSoftKeyboard(insets.bottom);
  const { rpc, socket } = useMachine();
  const ready = rpc !== null && socket.status === "ready";
  const [tiles, setTiles] = useState<readonly PageTile[]>([]);
  const [page, setPage] = useState<PageSize | null>(null);
  const [address, setAddress] = useState("");
  const [title, setTitle] = useState("");
  const [editing, setEditing] = useState(false);
  const [noPage, setNoPage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const [typed, setTyped] = useState("");
  const [frame, setFrame] = useState({ width: 0, height: 0 });
  const [mode, setMode] = useState<"mobile" | "desktop">("mobile");
  /** Elements the page keeps in place, drawn over the scrolling page. */
  const [overlays, setOverlays] = useState<readonly BrowserOverlay[]>([]);
  const [background, setBackground] = useState<string | null>(null);

  /** Requests run one after another, in the order asked. */
  const chain = useRef<Promise<void>>(Promise.resolve());
  const waiting = useRef(0);
  const claimed = useRef(false);
  const left = useRef(false);
  const seq = useRef(0);
  const urlRef = useRef("");
  const lastView = useRef(0);
  const editingRef = useRef(false);
  editingRef.current = editing;
  const taken = useRef(onTakenByDesktop);
  taken.current = onTakenByDesktop;
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const frameRef = useRef(frame);
  frameRef.current = frame;
  const pageRef = useRef(page);
  pageRef.current = page;
  const tilesRef = useRef(tiles);
  tilesRef.current = tiles;

  /**
   * Where the reader is on the page, in screen px from its top. This is the
   * truth while the reader scrolls; the desktop's page follows it.
   */
  const scroll = useRef(new Animated.Value(0)).current;
  const scrollNow = useRef(0);
  /** The reader moved since the desktop was last told where they are. */
  const dirty = useRef(false);
  const syncing = useRef(false);
  const touching = useRef(false);
  const coasting = useRef(false);
  const dragStart = useRef(0);
  const moved = useRef(false);
  /** The way the reader is moving: 1 down the page, -1 up, 0 still. */
  const heading = useRef<-1 | 0 | 1>(0);
  const peeking = useRef(false);
  /**
   * Places already asked for on this page. A place the page can't actually
   * scroll to never becomes "seen", and must not be asked for forever.
   */
  const peeked = useRef(new Set<number>());
  /** Panel pages: drag not yet sent, and where the drag began. */
  const panelDrag = useRef(0);
  const panelLast = useRef(0);
  const panelBusy = useRef(false);
  const dragPoint = useRef({ x: 0.5, y: 0.5 });

  /** Screen px per page px. */
  const scale = page && frame.width > 0 ? frame.width / page.width : 0;
  const scaleRef = useRef(scale);
  scaleRef.current = scale;

  const setScroll = useCallback((px: number) => {
    scrollNow.current = px;
    scroll.setValue(px);
  }, [scroll]);

  /** Do one thing in the tab (or just look) and take in the page that comes back. */
  const run = useCallback((action: BrowserAction, extra: Extra = {}, quiet = false): Promise<void> => {
    if (!rpc || !ready) {
      if (!quiet) setError("Not connected to your desktop.");
      return Promise.resolve();
    }
    waiting.current += 1;
    const next = chain.current.then(async () => {
      try {
        if (left.current) return;
        if (!quiet) setWorking(true);
        const claim = !claimed.current;
        const area = frameRef.current;
        const mobile = modeRef.current === "mobile" && area.width >= 240 && area.height >= 240
          ? { width: area.width, height: area.height }
          : null;
        const result = await browserAct(rpc, tabId, action, { ...extra, mobile, ...(claim ? { claim: true } : {}) });
        claimed.current = true;
        if (left.current) return;
        if (result.kind === "no_page") {
          setNoPage(result.message || "No page is open in this tab yet.");
          setTiles([]);
          setOverlays([]);
          setPage(null);
          setTitle("");
          setScroll(0);
        } else if (result.kind === "page") {
          const peek = action === "peek";
          const fresh = CHANGES_PAGE.has(action) || result.url !== urlRef.current;
          if (fresh) peeked.current.clear();
          setNoPage(null);
          if (!peek) {
            lastView.current = Date.now();
            urlRef.current = result.url;
            setTitle(result.title);
            setBackground(result.background);
            setOverlays((previous) =>
              previous.length === result.overlays.length && previous.every((o, i) => o.image === result.overlays[i]!.image && o.y === result.overlays[i]!.y)
                ? previous
                : result.overlays);
            if (!editingRef.current) setAddress(result.url);
            setPage((previous) =>
              previous && previous.width === result.width && previous.height === result.height && previous.full === result.full
                ? previous
                : { width: result.width, height: result.height, full: result.full });
          }
          const size = { width: result.width, height: result.height, full: result.full };
          const px = area.width > 0 ? area.width / size.width : 0;
          const panel = !windowScrolls(size.full, size.height);
          // The page moved by itself, a new page loaded, or a panel page was
          // scrolled: the reader is wherever the page now is. Otherwise the
          // reader's own position stands and the page follows it.
          const idle = !touching.current && !coasting.current && !dirty.current;
          if (!peek && px > 0 && (fresh || panel || action === "scroll" || (idle && action === "view"))) {
            setScroll(clampTop(result.top, size.full, size.height) * px);
          }
          if (result.picture) {
            seq.current += 1;
            const tile: PageTile = {
              top: result.top,
              height: size.height,
              uri: `data:image/jpeg;base64,${result.picture.image}`,
              seq: seq.current,
            };
            const near = px > 0 ? scrollNow.current / px : result.top;
            // A panel page looks different at the same place after every
            // scroll, so nothing seen earlier can be kept.
            setTiles((previous) => (fresh || panel ? addTile(previous.filter((t) => t.top === tile.top).slice(-1), tile, near) : addTile(previous, tile, near)));
          }
        }
        setError(null);
      } catch (failure) {
        if ((failure as { code?: unknown } | null)?.code === DESKTOP_IN_CONTROL_CODE) {
          // The desktop took the tab back: leave for the list of tabs.
          left.current = true;
          taken.current?.();
          return;
        }
        if (!quiet) setError(messageOf(failure));
      } finally {
        waiting.current -= 1;
        if (waiting.current === 0) setWorking(false);
      }
    });
    chain.current = next;
    return next;
  }, [rpc, ready, tabId, setScroll]);
  const runRef = useRef(run);
  runRef.current = run;

  /**
   * Keep the desktop's page with the reader. While they are moving over
   * page that is already known here, the desktop isn't asked to follow every
   * step: the screens ahead are fetched instead, and the page is brought to
   * the reader once they stop. One request at a time.
   */
  const sync = useCallback(() => {
    const px = scaleRef.current;
    const size = pageRef.current;
    if (syncing.current || peeking.current || !dirty.current || px <= 0 || !size) return;
    const top = scrollNow.current / px;
    const moving = touching.current || coasting.current;
    if (moving && covers(tilesRef.current, top, Math.min(top + size.height, size.full))) {
      const ahead = nextPeek(tilesRef.current, top, size.height, size.full, heading.current);
      if (ahead === null || peeked.current.has(ahead)) return;
      peeked.current.add(ahead);
      peeking.current = true;
      void runRef.current("peek", { top: ahead }, true).finally(() => {
        peeking.current = false;
        sync();
      });
      return;
    }
    dirty.current = false;
    syncing.current = true;
    void runRef.current("scroll_to", { top }, true).finally(() => {
      syncing.current = false;
      sync();
    });
  }, []);

  useFocusEffect(useCallback(() => {
    left.current = false;
    claimed.current = false;
    void run("view");
    const timer = setInterval(() => {
      // Only while nothing is queued, on screen, and the reader is still.
      if (waiting.current > 0 || AppState.currentState !== "active" || editingRef.current) return;
      if (touching.current || coasting.current || dirty.current) return;
      if (Date.now() - lastView.current >= REFRESH_MS) {
        void run("view", {}, true);
        return;
      }
      // Fetch the screen ahead, so scrolling finds the page already there.
      const size = pageRef.current;
      const px = scaleRef.current;
      if (!size || px <= 0 || !windowScrolls(size.full, size.height)) return;
      const ahead = nextPeek(tilesRef.current, scrollNow.current / px, size.height, size.full, heading.current);
      if (ahead !== null && !peeked.current.has(ahead)) {
        peeked.current.add(ahead);
        void run("peek", { top: ahead }, true);
      }
    }, TICK_MS);
    return () => {
      clearInterval(timer);
      // Tell the desktop the tab is free again (best effort).
      if (!left.current && rpc && ready) void browserAct(rpc, tabId, "release").catch(() => {});
      left.current = true;
    };
  }, [run, rpc, ready, tabId]));

  // A different tab starts over.
  useEffect(() => {
    setTiles([]);
    setOverlays([]);
    setBackground(null);
    setPage(null);
    setNoPage(null);
    setError(null);
    setAddress("");
    urlRef.current = "";
    dirty.current = false;
    setScroll(0);
  }, [tabId, setScroll]);

  // Switching view, or the page area changing size, lays the page out again:
  // what was seen at the old layout no longer fits.
  const area = `${mode}:${Math.round(frame.width)}x${Math.round(frame.height)}`;
  const firstArea = useRef(true);
  useEffect(() => {
    if (firstArea.current) {
      firstArea.current = false;
      return;
    }
    if (frame.width <= 0) return;
    setTiles([]);
    setOverlays([]);
    dirty.current = false;
    void run("view");
  }, [area, run]); // eslint-disable-line react-hooks/exhaustive-deps

  /** Panel pages: send the drag gathered so far as one scroll under the finger. */
  const pumpPanel = useCallback((least: number) => {
    const size = pageRef.current;
    const px = scaleRef.current;
    if (!size || px <= 0 || panelBusy.current || Math.abs(panelDrag.current) < least) return;
    const dy = panelDrag.current;
    panelDrag.current = 0;
    panelBusy.current = true;
    const by = Math.max(-20, Math.min(20, -dy / (size.height * px)));
    void runRef.current("scroll", { by, ...dragPoint.current }, true).finally(() => {
      panelBusy.current = false;
      pumpPanel(1);
    });
  }, []);

  const touch = useMemo(
    () =>
      PanResponder.create({
        onStartShouldSetPanResponder: () => true,
        onMoveShouldSetPanResponder: (_, gesture) => Math.abs(gesture.dy) > TAP_SLOP_PX,
        onPanResponderGrant: (event) => {
          touching.current = true;
          moved.current = false;
          // A finger down stops a flick where it is.
          scroll.stopAnimation();
          coasting.current = false;
          dragStart.current = scrollNow.current;
          panelLast.current = 0;
          const size = pageRef.current;
          const px = scaleRef.current;
          if (size && px > 0) {
            dragPoint.current = {
              x: Math.min(1, Math.max(0, event.nativeEvent.locationX / (size.width * px))),
              y: Math.min(1, Math.max(0, event.nativeEvent.locationY / (size.height * px))),
            };
          }
        },
        onPanResponderMove: (_, gesture) => {
          const size = pageRef.current;
          const px = scaleRef.current;
          if (!size || px <= 0) return;
          if (!moved.current && Math.abs(gesture.dy) < TAP_SLOP_PX) return;
          moved.current = true;
          if (windowScrolls(size.full, size.height)) {
            // The page moves under the finger at once; the desktop follows.
            const top = Math.min(maxTop(size.full, size.height) * px, Math.max(0, dragStart.current - gesture.dy));
            if (top !== scrollNow.current) heading.current = top > scrollNow.current ? 1 : -1;
            scrollNow.current = top;
            scroll.setValue(top);
            dirty.current = true;
            sync();
          } else {
            panelDrag.current += gesture.dy - panelLast.current;
            panelLast.current = gesture.dy;
            pumpPanel(16);
          }
        },
        onPanResponderRelease: (event, gesture) => {
          touching.current = false;
          const size = pageRef.current;
          const px = scaleRef.current;
          if (!size || px <= 0) return;
          if (!moved.current) {
            // A tap: a point of the visible screen. The page is brought to
            // where the reader is first, so the tap lands on what they see.
            const x = Math.min(1, Math.max(0, event.nativeEvent.locationX / (size.width * px)));
            const y = Math.min(1, Math.max(0, event.nativeEvent.locationY / (size.height * px)));
            if (dirty.current) sync();
            void runRef.current("tap", { x, y });
            return;
          }
          if (!windowScrolls(size.full, size.height)) {
            panelDrag.current += gesture.dy - panelLast.current;
            panelLast.current = 0;
            pumpPanel(1);
            return;
          }
          // A flick carries on and slows down, as a page does.
          const limit = maxTop(size.full, size.height) * px;
          if (Math.abs(gesture.vy) > 0.15) {
            coasting.current = true;
            const id = scroll.addListener(({ value }) => {
              const top = Math.min(limit, Math.max(0, value));
              scrollNow.current = top;
              dirty.current = true;
              sync();
              if (top !== value) {
                scroll.stopAnimation();
                scroll.setValue(top);
              }
            });
            Animated.decay(scroll, { velocity: -gesture.vy, deceleration: 0.996, useNativeDriver: false }).start(() => {
              scroll.removeListener(id);
              coasting.current = false;
              dirty.current = true;
              sync();
            });
          } else {
            dirty.current = true;
            sync();
          }
        },
        onPanResponderTerminate: () => {
          touching.current = false;
          dirty.current = true;
          sync();
        },
      }),
    [scroll, sync, pumpPanel],
  );

  const go = () => {
    const url = address.trim();
    setEditing(false);
    if (url !== "") void run("open", { url });
  };
  const sendTyped = () => {
    if (typed === "") return;
    const text = typed;
    setTyped("");
    void run("type", { text });
  };
  const press = (key: BrowserKey) => void run("press", { key });
  /** The Page buttons: one screen up or down. */
  const pageBy = (screens: number) => {
    const size = pageRef.current;
    const px = scaleRef.current;
    if (!size || px <= 0) return;
    if (!windowScrolls(size.full, size.height)) {
      void run("scroll", { by: screens });
      return;
    }
    const target = clampTop(scrollNow.current / px + screens * size.height, size.full, size.height) * px;
    scrollNow.current = target;
    Animated.timing(scroll, { toValue: target, duration: 160, useNativeDriver: false }).start();
    dirty.current = true;
    sync();
  };
  const onFrame = (event: LayoutChangeEvent) => {
    const { width, height } = event.nativeEvent.layout;
    setFrame((previous) => (previous.width === width && previous.height === height ? previous : { width, height }));
  };

  const ordered = useMemo(() => [...tiles].sort((a, b) => a.seq - b.seq), [tiles]);
  const boxHeight = page && scale > 0 ? Math.min(frame.height, page.height * scale) : 0;
  const up = useMemo(() => Animated.multiply(scroll, -1), [scroll]);

  return (
    <View style={[styles.frame, { paddingBottom: keyboard.height }]}>
      <View style={[styles.bar, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
        <IconButton name="arrow-left" label="Back" size="sm" disabled={!ready} onPress={() => void run("back")} />
        <IconButton name="chevron-right" label="Forward" size="sm" disabled={!ready} onPress={() => void run("forward")} />
        <IconButton name="refresh" label="Reload" size="sm" disabled={!ready} onPress={() => void run("reload")} />
        <FieldInput
          containerStyle={styles.address}
          style={styles.addressInput}
          value={address}
          onChangeText={setAddress}
          onFocus={() => setEditing(true)}
          onBlur={() => setEditing(false)}
          onSubmitEditing={go}
          placeholder="Search or enter an address"
          accessibilityLabel="Address"
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="url"
          returnKeyType="go"
          selectTextOnFocus
        />
        {working ? <ActivityIndicator size="small" color={theme.colors.textMuted} /> : null}
      </View>
      {title ? (
        <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.title}>{title}</AppText>
      ) : null}
      {error ? <AppText variant="caption" tone="danger" accessibilityRole="alert" style={styles.title}>{error}</AppText> : null}
      <View style={styles.page} onLayout={onFrame}>
        {noPage ? (
          <View style={styles.center}>
            <AppText style={styles.centerText}>{noPage.replace(/Run `dragent browser[^`]*`\.?/gu, "").trim() || "No page is open in this tab yet."}</AppText>
            <AppText variant="caption" tone="textMuted" style={styles.centerText}>
              Enter an address above, such as localhost:3000.
            </AppText>
          </View>
        ) : !page || scale <= 0 || tiles.length === 0 ? (
          <View style={styles.center}>
            <ActivityIndicator color={theme.colors.accent} />
            <AppText variant="caption" tone="textMuted" style={styles.centerText}>
              Showing the tab on your desktop…
            </AppText>
          </View>
        ) : (
          // This box is the browser's screen: it never moves and it takes the
          // touches. The page moves inside it.
          <View
            {...touch.panHandlers}
            accessible
            accessibilityLabel="Page. Tap to click, drag to scroll."
            style={[styles.screen, { width: frame.width, height: boxHeight, backgroundColor: background ?? theme.colors.background }]}
          >
            <Animated.View pointerEvents="none" style={{ transform: [{ translateY: up }] }}>
              {ordered.map((tile) => (
                <Image
                  key={tile.seq}
                  source={{ uri: tile.uri }}
                  style={[styles.tile, { top: tile.top * scale, width: frame.width, height: tile.height * scale }]}
                  resizeMode="stretch"
                  fadeDuration={0}
                />
              ))}
            </Animated.View>
            {/* What the page keeps in place stays in place here too. */}
            {overlays.map((overlay, index) => (
              <Image
                key={`${index}:${overlay.y}:${overlay.image.length}`}
                pointerEvents="none"
                source={{ uri: `data:image/jpeg;base64,${overlay.image}` }}
                style={[styles.tile, { left: overlay.x * scale, top: overlay.y * scale, width: overlay.width * scale, height: overlay.height * scale }]}
                resizeMode="stretch"
                fadeDuration={0}
              />
            ))}
          </View>
        )}
      </View>
      <View style={[styles.dock, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
        <ScrollView horizontal keyboardShouldPersistTaps="always" showsHorizontalScrollIndicator={false} contentContainerStyle={styles.keys}>
          <Button
            label={mode === "mobile" ? "Mobile view" : "Desktop view"}
            accessibilityLabel={mode === "mobile" ? "Mobile view. Switch to desktop view." : "Desktop view. Switch to mobile view."}
            size="sm"
            variant="ghost"
            disabled={!ready}
            onPress={() => setMode((current) => (current === "mobile" ? "desktop" : "mobile"))}
          />
          <Button label="↑ Page" size="sm" variant="ghost" disabled={!ready} onPress={() => pageBy(-0.8)} />
          <Button label="↓ Page" size="sm" variant="ghost" disabled={!ready} onPress={() => pageBy(0.8)} />
          <Button label="Enter" size="sm" variant="ghost" disabled={!ready} onPress={() => press("Enter")} />
          <Button label="Tab" size="sm" variant="ghost" disabled={!ready} onPress={() => press("Tab")} />
          <Button label="⌫" accessibilityLabel="Backspace" size="sm" variant="ghost" disabled={!ready} onPress={() => press("Backspace")} />
          <Button label="Esc" size="sm" variant="ghost" disabled={!ready} onPress={() => press("Escape")} />
          <Button label="Hide keyboard" size="sm" variant="ghost" onPress={() => Keyboard.dismiss()} />
        </ScrollView>
        <View style={styles.typeRow}>
          <FieldInput
            containerStyle={styles.flex}
            style={styles.typeInput}
            value={typed}
            onChangeText={setTyped}
            onSubmitEditing={sendTyped}
            placeholder="Tap a field on the page, then type here"
            accessibilityLabel="Text to type into the page"
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="send"
            blurOnSubmit={false}
          />
          <Button label="Type" size="sm" style={styles.tap} disabled={!ready || typed === ""} onPress={sendTyped} />
        </View>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  flex: { flex: 1, minWidth: 0 },
  bar: {
    flexDirection: "row",
    alignItems: "center",
    gap: 2,
    paddingHorizontal: spacing.xs,
    paddingVertical: 4,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  address: { flex: 1, minWidth: 0, marginLeft: 4, marginRight: 4 },
  addressInput: { minHeight: 36, borderRadius: radii.pill },
  title: { paddingHorizontal: spacing.md, paddingTop: 4 },
  page: { flex: 1, minHeight: 0, overflow: "hidden" },
  screen: { overflow: "hidden" },
  tile: { position: "absolute", left: 0 },
  center: { flex: 1, alignItems: "center", justifyContent: "center", gap: spacing.sm, padding: spacing.xl },
  centerText: { textAlign: "center" },
  dock: { borderTopWidth: StyleSheet.hairlineWidth, paddingHorizontal: spacing.sm, paddingVertical: 4, gap: 2 },
  keys: { flexDirection: "row", alignItems: "center" },
  typeRow: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  typeInput: { minHeight: 40 },
  tap: { minHeight: 40 },
});
