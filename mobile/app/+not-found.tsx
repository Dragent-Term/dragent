import { router } from "expo-router";
import { EmptyState, Icon } from "../src/components";
import { ScrollScreen } from "../src/layout";

export default function NotFound() {
  return <ScrollScreen><EmptyState icon={<Icon name="folder" size={24} />} title="This screen is unavailable" message="Return to your saved sessions to continue."
    actionLabel="Your sessions" onAction={() => router.replace("/")} /></ScrollScreen>;
}
