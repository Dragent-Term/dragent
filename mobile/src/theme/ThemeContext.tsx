import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { useColorScheme } from "react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { BUNDLED_THEMES } from "./catalog.generated";
import {
  CURATED_THEME_IDS,
  DEFAULT_THEME_ID,
  normalizeSelection,
  resolveAppTheme,
} from "./derive";
import type { AppTheme, ThemeStorage, UseThemeResult } from "./types";

export const THEME_SELECTION_KEY = "dragent.theme.selection.v1";

const defaultStorage: ThemeStorage = {
  getItem: (key) => AsyncStorage.getItem(key),
  setItem: (key, value) => AsyncStorage.setItem(key, value),
};

export interface ThemeProviderProps {
  children: ReactNode;
  /** Initial selection before persisted state loads (defaults to dark). */
  initialSelection?: string | undefined;
  /** Injectable storage for tests; defaults to AsyncStorage. Context owns all reads/writes. */
  storage?: ThemeStorage | undefined;
  /** Overrides `useColorScheme()` in tests or previews. */
  colorSchemeOverride?: "light" | "dark" | null | undefined;
}

const ThemeContext = createContext<UseThemeResult | null>(null);

/**
 * Curated four-theme provider. Colors are dimension-independent so phone,
 * tablet, foldable, window-resizing, and large-font settings share one code
 * path. This context is the only owner of theme persistence (AsyncStorage).
 * Stored legacy ids (`system`, `github_dark`, `custom`, typos) normalize to
 * the curated catalog so selection is always one of the four themes.
 */
export function ThemeProvider({
  children,
  initialSelection,
  storage,
  colorSchemeOverride,
}: ThemeProviderProps): React.JSX.Element {
  const store = storage ?? defaultStorage;
  const systemScheme = useColorScheme();
  const osScheme = colorSchemeOverride !== undefined ? colorSchemeOverride : systemScheme;

  const themes = useMemo<readonly AppTheme[]>(() => BUNDLED_THEMES, []);

  const [selection, setSelection] = useState<string>(() =>
    CURATED_THEME_IDS.includes(initialSelection ?? "") ? (initialSelection as string) : DEFAULT_THEME_ID,
  );
  const [isReady, setIsReady] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const persisted = await store.getItem(THEME_SELECTION_KEY).catch(() => null);
        if (cancelled) return;
        if (typeof persisted === "string" && persisted.length > 0) {
          const normalized = normalizeSelection(persisted, osScheme);
          setSelection(normalized);
          if (normalized !== persisted) {
            await store.setItem(THEME_SELECTION_KEY, normalized).catch(() => {});
          }
        }
      } catch {
        if (!cancelled) setError("Could not load the saved theme; using the default.");
      } finally {
        if (!cancelled) setIsReady(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [store, osScheme]);

  const theme = useMemo<AppTheme>(
    () => resolveAppTheme(selection, osScheme, themes),
    [selection, osScheme, themes],
  );

  const setThemeId = useCallback(
    async (id: string): Promise<void> => {
      if (!themes.some((entry) => entry.id === id)) {
        const message = `Unknown theme "${id}"`;
        setError(message);
        throw new Error(message);
      }
      setSelection(id);
      setError(null);
      try {
        await store.setItem(THEME_SELECTION_KEY, id);
      } catch {
        setError("Could not save the theme selection.");
      }
    },
    [store, themes],
  );

  const value = useMemo<UseThemeResult>(
    () => ({ theme, themes, selection, setThemeId, isReady, error }),
    [theme, themes, selection, setThemeId, isReady, error],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

/** Access the active curated theme and theme controls. Must be used under `ThemeProvider`. */
export function useTheme(): UseThemeResult {
  const value = useContext(ThemeContext);
  if (!value) throw new Error("useTheme must be used within ThemeProvider");
  return value;
}
