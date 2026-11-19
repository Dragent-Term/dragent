import { useInOptionRow } from "./OptionRow";
import { ThemedStatusBar } from "./ThemedStatusBar";
import { useCallback, useEffect, useMemo, useRef, useState, type ComponentRef, type ReactNode } from "react";
import {
  Modal,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
  useWindowDimensions,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useTheme } from "../theme";
import { AppText } from "./AppText";
import { Control } from "./Control";
import { Icon, type IconName } from "./Icon";
import { SearchField } from "./SearchField";

export interface SelectPopoverOption<T extends string> {
  id: T;
  label: string;
  hint?: string;
  /** Shown before the label, in the menu and on the trigger when chosen (e.g. an agent's mark). */
  leading?: ReactNode;
}

export interface SelectPopoverProps<T extends string> {
  /** Accessible trigger name, e.g. "Project". Also prefixes the trigger label. */
  label: string;
  /** Desktop-style menu caption, e.g. "PROJECTS". */
  caption: string;
  options: readonly SelectPopoverOption<T>[];
  value: T;
  onChange: (id: T) => void;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Show a filter input at the top of the menu. Off for short fixed lists. */
  searchable?: boolean;
  searchPlaceholder?: string;
  /** Static trigger text for action menus with no selection (e.g. "More"). Defaults to the selected option label. */
  triggerLabel?: string;
  /** A round, borderless trigger sized to its label (a message box's controls). */
  pill?: boolean;
  /** A full-width settings row: icon, label on the left, the choice on the right. */
  row?: { icon: IconName };
}

interface Anchor {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * One desktop-style review selector: a compact trigger (current selection +
 * down chevron) opening an anchored overlay menu in a native Modal portal so
 * it can never be clipped by a FlatList. Generic over string option ids; all
 * review/project/state/scope knowledge stays with the caller.
 */
export function SelectPopover<T extends string>({
  label,
  caption,
  options,
  value,
  onChange,
  open,
  onOpenChange,
  searchable = false,
  searchPlaceholder,
  triggerLabel,
  pill = false,
  row,
}: SelectPopoverProps<T>) {
  const { theme } = useTheme();
  const { width: windowWidth, height: windowHeight } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const anchorHost = useRef<ComponentRef<typeof View>>(null);
  const triggerRef = useRef<ComponentRef<typeof View>>(null);
  const inRow = useInOptionRow();
  const menuRef = useRef<ComponentRef<typeof View>>(null);
  const [anchor, setAnchor] = useState<Anchor | null>(null);
  const [query, setQuery] = useState("");
  const restoreFocus = useRef(false);
  const wasOpen = useRef(false);
  const openedDims = useRef<string | null>(null);

  const selectedOption = useMemo(() => options.find((option) => option.id === value) ?? null, [options, value]);
  const selectedLabel = selectedOption?.label ?? "";
  const displayLabel = triggerLabel ?? selectedLabel;
  const triggerAccessibilityLabel = displayLabel && displayLabel !== label ? `${label}: ${displayLabel}` : label;

  const filtered = useMemo(() => {
    const search = query.trim().toLowerCase();
    if (!search) return options;
    return options.filter((option) =>
      option.label.toLowerCase().includes(search)
      || (option.hint ?? "").toLowerCase().includes(search),
    );
  }, [options, query]);
  const selectedIndex = Math.max(0, filtered.findIndex((option) => option.id === value));

  const openMenu = useCallback(() => {
    restoreFocus.current = false;
    const node = anchorHost.current;
    if (node && typeof node.measureInWindow === "function") {
      node.measureInWindow((x, y, width, height) => {
        setAnchor({ x, y, width, height });
        onOpenChange(true);
      });
    } else {
      setAnchor(null);
      onOpenChange(true);
    }
  }, [onOpenChange]);

  const requestClose = useCallback((restore: boolean) => {
    restoreFocus.current = restore;
    onOpenChange(false);
  }, [onOpenChange]);

  const select = useCallback((id: T) => {
    restoreFocus.current = Platform.OS === "web";
    onChange(id);
    onOpenChange(false);
  }, [onChange, onOpenChange]);

  // The search input stays mounted while filtering; only its value changes.
  useEffect(() => {
    if (open) setQuery("");
  }, [open]);

  // A stale anchor is worse than a closed menu: dismiss on rotation/resize.
  const dimsKey = `${windowWidth}x${windowHeight}`;
  useEffect(() => {
    if (!open) {
      openedDims.current = null;
      return;
    }
    if (openedDims.current === null) {
      openedDims.current = dimsKey;
      return;
    }
    if (openedDims.current !== dimsKey) {
      openedDims.current = dimsKey;
      restoreFocus.current = false;
      onOpenChange(false);
    }
  }, [dimsKey, open, onOpenChange]);

  // Restore the trigger when the menu closes, unless focus has since moved
  // to a newly focused route — never steal it back.
  useEffect(() => {
    if (wasOpen.current && !open && Platform.OS === "web") {
      if (restoreFocus.current) {
        const active = document.activeElement;
        if (!active || active === document.body) {
          const trigger = triggerRef.current as unknown as { focus?: () => void } | null;
          trigger?.focus?.();
        }
      }
      restoreFocus.current = false;
    }
    wasOpen.current = open;
  }, [open]);

  // Focus the selected option on web so arrows/Enter work immediately.
  useEffect(() => {
    if (!open || Platform.OS !== "web") return;
    const frame = requestAnimationFrame(() => {
      const menu = menuRef.current as unknown as HTMLElement | null;
      if (!menu) return;
      const selected = menu.querySelector<HTMLElement>('[aria-checked="true"]');
      const fallback = menu.querySelector<HTMLElement>('input, [role="menuitemradio"], [role="radio"]');
      (selected ?? fallback)?.focus?.();
    });
    return () => cancelAnimationFrame(frame);
  }, [open]);

  // Arrow keys open the trigger on web (Enter/Space already press it).
  useEffect(() => {
    if (Platform.OS !== "web" || open) return;
    const host = anchorHost.current as unknown as HTMLElement | null;
    if (!host) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        openMenu();
      }
    };
    host.addEventListener("keydown", onKey);
    return () => host.removeEventListener("keydown", onKey);
  }, [open, openMenu]);

  // Arrow/Home/End move between options; Enter/Space activate natively.
  useEffect(() => {
    if (!open || Platform.OS !== "web") return;
    const menu = menuRef.current as unknown as HTMLElement | null;
    if (!menu) return;
    const onKey = (event: KeyboardEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest("input, textarea")) {
        if (event.key !== "Escape") return;
        event.preventDefault();
        event.stopPropagation();
        requestClose(true);
        return;
      }
      const radios = Array.from(menu.querySelectorAll<HTMLElement>('[role="menuitemradio"], [role="radio"]'));
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        requestClose(true);
        return;
      }
      if (radios.length === 0) return;
      const index = radios.findIndex((radio) => target !== null && (radio === target || radio.contains(target)));
      const focusAt = (next: number) => {
        event.preventDefault();
        event.stopPropagation();
        radios[next]?.focus();
      };
      switch (event.key) {
        case "ArrowDown": focusAt(index < 0 ? 0 : (index + 1) % radios.length); break;
        case "ArrowUp": focusAt(index < 0 ? radios.length - 1 : (index + radios.length - 1) % radios.length); break;
        case "Home": focusAt(0); break;
        case "End": focusAt(radios.length - 1); break;
        default: break;
      }
    };
    menu.addEventListener("keydown", onKey, true);
    return () => menu.removeEventListener("keydown", onKey, true);
  }, [open, filtered.length, requestClose]);

  // Dismiss on intentional outside scrolling. Focus and scrollIntoView may
  // also emit scroll events while opening/searching; those must keep it open.
  useEffect(() => {
    if (!open || Platform.OS !== "web") return;
    const onDocumentKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        requestClose(true);
      }
    };
    const onWheel = (event: Event) => {
      const menu = menuRef.current as unknown as HTMLElement | null;
      if (menu && event.target instanceof Node && menu.contains(event.target)) return;
      requestClose(false);
    };
    document.addEventListener("keydown", onDocumentKey);
    window.addEventListener("wheel", onWheel, { capture: true, passive: true });
    return () => {
      document.removeEventListener("keydown", onDocumentKey);
      window.removeEventListener("wheel", onWheel, true);
    };
  }, [open, requestClose]);

  const menuWidth = Math.max(160, Math.min(row ? 340 : 280, windowWidth - insets.left - insets.right - 16));
  const anchorLeft = anchor?.x ?? (windowWidth - menuWidth) / 2;
  const anchorTop = anchor?.y ?? insets.top + 8;
  const left = Math.max(insets.left + 8, Math.min(anchorLeft, windowWidth - insets.right - 8 - menuWidth));
  const belowTop = anchorTop + (anchor?.height ?? 0) + 6;
  const roomBelow = windowHeight - insets.bottom - 8 - belowTop;
  const roomAbove = anchorTop - 6 - insets.top - 8;
  const showAbove = roomBelow < 160 && roomAbove > roomBelow;
  const chromeHeight = (searchable ? 52 : 0) + 34 + 16;
  const maxMenuHeight = Math.max(132, Math.min(260 + chromeHeight, Math.max(0, showAbove ? roomAbove : roomBelow)));

  return (
    <View ref={anchorHost} collapsable={false} style={row ? styles.rowHost : [styles.host, pill ? styles.pillHost : null, inRow ? styles.fitHost : null]}>
      {row ? (
        <Control
          ref={triggerRef}
          accessibilityRole="button"
          aria-haspopup="menu"
          accessibilityLabel={triggerAccessibilityLabel}
          accessibilityState={{ expanded: open }}
          onPress={() => { if (open) requestClose(true); else openMenu(); }}
          style={({ pressed, hovered }) => [
            styles.rowTrigger,
            pressed || hovered || open ? { backgroundColor: theme.colors.surfaceRaised } : null,
          ]}
        >
          <View style={[styles.rowIcon, { backgroundColor: theme.colors.surfaceRaised }]}>
            <Icon name={row.icon} size={15} tone="textMuted" />
          </View>
          <AppText variant="body" style={styles.rowLabel}>{label}</AppText>
          <View style={styles.rowValue}>
            {selectedOption?.leading ?? null}
            <AppText variant="body" tone="textMuted" numberOfLines={1} style={styles.rowValueText}>{displayLabel}</AppText>
          </View>
          <Icon name="chevron-down" size={14} tone="textSubtle" />
        </Control>
      ) : (
      <Control
        ref={triggerRef}
        accessibilityRole="button"
        aria-haspopup="menu"
        accessibilityLabel={triggerAccessibilityLabel}
        accessibilityState={{ expanded: open }}
        onPress={() => { if (open) requestClose(true); else openMenu(); }}
        style={({ pressed, hovered }) => [
          styles.trigger,
          pill ? styles.pillTrigger : null,
          {
            borderColor: open ? theme.colors.accent : pill ? "transparent" : theme.colors.border,
            backgroundColor: pressed || hovered || (pill && open) ? (pill ? theme.colors.selection : theme.colors.surfaceRaised) : theme.colors.surface,
          },
        ]}
      >
        {selectedOption?.leading ?? null}
        <AppText variant="caption" numberOfLines={1} style={inRow || pill ? styles.fitValue : styles.triggerValue}>{displayLabel}</AppText>
        <Icon name="chevron-down" size={14} />
      </Control>
      )}
      <Modal visible={open} transparent animationType="fade" onRequestClose={() => requestClose(true)}>
        <ThemedStatusBar />
        <View style={styles.portal}>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`Dismiss ${label} menu`}
            onPress={() => requestClose(true)}
            style={styles.backdrop}
          />
          <View
            ref={menuRef}
            accessibilityRole="menu"
            accessibilityLabel={caption}
            style={[
              styles.menu,
              {
                backgroundColor: theme.colors.surface,
                borderColor: theme.colors.border,
                shadowColor: theme.colors.text,
                left,
                width: menuWidth,
                maxHeight: maxMenuHeight,
                ...(showAbove ? { bottom: windowHeight - anchorTop + 6 } : { top: belowTop }),
              },
            ]}
          >
            <AppText variant="micro" tone="textSubtle" style={styles.caption}>{caption}</AppText>
            {searchable ? (
              <SearchField
                value={query}
                onChangeText={setQuery}
                placeholder={searchPlaceholder ?? `Search ${label.toLowerCase()}`}
                label={searchPlaceholder ?? `Search ${label.toLowerCase()}`}
              />
            ) : null}
            <ScrollView style={styles.list} keyboardShouldPersistTaps="handled">
              {filtered.map((option, index) => {
                const selected = value === option.id;
                const hint = option.hint && option.hint !== option.label ? option.hint : null;
                return (
                  <Control
                    key={option.id}
                    accessibilityRole={Platform.OS === "web" ? "menuitemradio" : "radio"}
                    accessibilityState={{ checked: selected }}
                    {...(Platform.OS === "web" ? { tabIndex: index === selectedIndex ? 0 as const : -1 as const } : {})}
                    accessibilityLabel={hint ? `${option.label}, ${hint}` : option.label}
                    onPress={() => select(option.id)}
                    style={({ pressed, hovered }) => [
                      styles.option,
                      selected || pressed || hovered ? { backgroundColor: theme.colors.surfaceRaised } : null,
                    ]}
                  >
                    {option.leading ?? null}
                    <AppText variant="body" numberOfLines={1} style={styles.optionLabel}>{option.label}</AppText>
                    {hint ? (
                      <AppText variant="caption" tone="textSubtle" numberOfLines={1} style={styles.hint}>{hint}</AppText>
                    ) : null}
                    <View style={styles.check}>
                      {selected ? <Icon name="check" size={14} tone="accent" /> : null}
                    </View>
                  </Control>
                );
              })}
              {filtered.length === 0 ? (
                <AppText variant="caption" tone="textSubtle" style={styles.empty}>No matches</AppText>
              ) : null}
            </ScrollView>
          </View>
        </View>
      </Modal>
    </View>
  );
}

const styles = StyleSheet.create({
  host: { flex: 1, flexShrink: 1, flexBasis: 0, minWidth: 0, maxWidth: 200 },
  trigger: {
    minHeight: 36,
    flexDirection: "row",
    alignItems: "center",
    gap: 4,
    borderWidth: 1,
    borderRadius: 10,
    paddingHorizontal: 10,
    paddingVertical: 4,
  },
  triggerValue: { flex: 1, minWidth: 0, fontWeight: "500" },
  pillHost: { flex: 0, flexBasis: "auto", flexShrink: 1, maxWidth: 190 },
  // In an OptionRow: as wide as its label (the row scrolls instead).
  fitHost: { flexGrow: 0, flexShrink: 0, flexBasis: "auto", maxWidth: 260 },
  fitValue: { flexShrink: 1, minWidth: 0, fontWeight: "500" },
  pillTrigger: { minHeight: 38, borderRadius: 19, paddingHorizontal: 12 },
  rowHost: { alignSelf: "stretch" },
  rowTrigger: { minHeight: 52, flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 14 },
  rowIcon: { width: 28, height: 28, borderRadius: 8, alignItems: "center", justifyContent: "center" },
  rowLabel: { flexShrink: 0 },
  rowValue: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", justifyContent: "flex-end", gap: 6 },
  rowValueText: { flexShrink: 1, textAlign: "right" },
  portal: { position: "absolute", top: 0, left: 0, right: 0, bottom: 0 },
  backdrop: { position: "absolute", top: 0, left: 0, right: 0, bottom: 0 },
  menu: {
    position: "absolute",
    borderWidth: 1,
    borderRadius: 12,
    padding: 8,
    gap: 4,
    shadowOpacity: 0.1,
    shadowRadius: 6,
    shadowOffset: { width: 0, height: 2 },
    elevation: 2,
  },
  caption: { paddingHorizontal: 10, paddingTop: 4 },
  list: { flexGrow: 0, flexShrink: 1 },
  option: {
    minHeight: 44,
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
  optionLabel: { flex: 1, minWidth: 0 },
  hint: { flexShrink: 1, maxWidth: "40%" },
  check: { width: 16, flexShrink: 0, alignItems: "center", justifyContent: "center" },
  empty: { paddingHorizontal: 10, paddingVertical: 12 },
});
