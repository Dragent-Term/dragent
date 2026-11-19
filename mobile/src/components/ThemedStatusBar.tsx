import { StatusBar } from "expo-status-bar";
import { useTheme } from "../theme";

/**
 * The status bar in the app theme's colours: white text over a dark theme,
 * black over a light one. A full-screen modal is its own iOS view controller
 * and sets its own status bar style, so every modal renders one of these
 * too, or opening it flips the status bar.
 */
export function ThemedStatusBar() {
  const { theme } = useTheme();
  return <StatusBar style={theme.mode === "dark" ? "light" : "dark"} animated />;
}
