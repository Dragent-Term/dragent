import { Alert, Platform } from "react-native";

interface ConfirmationOptions {
  title: string;
  message: string;
  actionLabel: string;
  actionStyle?: "default" | "destructive";
  onConfirm: () => void;
}

/** Native alerts and browser confirmation share the same action and copy. */
export function confirmAction({ title, message, actionLabel, actionStyle = "default", onConfirm }: ConfirmationOptions): void {
  if (Platform.OS === "web") {
    // React Native Web's Alert.alert is a no-op.
    if (typeof globalThis.confirm === "function" && globalThis.confirm(`${title}\n\n${message}`)) onConfirm();
    return;
  }
  Alert.alert(title, message, [
    { text: "Cancel", style: "cancel" },
    { text: actionLabel, style: actionStyle, onPress: onConfirm },
  ]);
}
