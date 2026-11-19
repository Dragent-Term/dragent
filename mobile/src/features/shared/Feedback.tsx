import { AppText } from "../../components";

export function Feedback({ message }: { message: string | null | undefined }) {
  if (!message) return null;
  return <AppText tone="danger" variant="caption" accessibilityRole="alert" accessibilityLiveRegion="polite">{message}</AppText>;
}
