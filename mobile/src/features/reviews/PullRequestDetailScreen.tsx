import { KeyboardAvoidingView, Platform, StyleSheet } from "react-native";
import { router } from "expo-router";
import { useTheme } from "../../theme";
import { PreviewHeader } from "../../preview/PreviewHeader";
import { usePreview } from "../../preview/PreviewContext";
import { selectPullRequest } from "../../preview/pullRequestSelectors";
import { PullRequestDetail } from "./PullRequestDetail";

/**
 * Route wrapper for `/preview/desktop/[desktopId]/review/[reviewId]`
 * (route file is root-owned). Phone back returns to the hub review view.
 */
export function PullRequestDetailScreen({ desktopId, reviewId }: { desktopId: string; reviewId: string }) {
  const { theme } = useTheme();
  const preview = usePreview();
  const review = selectPullRequest(preview, desktopId, reviewId);
  const backToHub = () =>
    router.navigate({ pathname: "/preview/desktop/[desktopId]", params: { desktopId, view: "review" } });

  return (
    <KeyboardAvoidingView
      style={[styles.frame, { backgroundColor: theme.colors.background }]}
      behavior={Platform.OS === "ios" ? "padding" : undefined}
    >
      <PreviewHeader
        title={review?.title ?? "Review unavailable"}
        subtitle="Sample review · local preview"
        onBack={backToHub}
        backLabel="Back to code review"
      />
      <PullRequestDetail desktopId={desktopId} reviewId={reviewId} />
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1 },
});
