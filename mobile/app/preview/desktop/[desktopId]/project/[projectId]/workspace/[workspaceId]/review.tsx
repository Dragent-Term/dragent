import { WorkspaceReviewScreen } from "../../../../../../../../src/features/reviews/WorkspaceReviewScreen";
import { usePreviewRouteParams } from "../../../../../../../../src/preview/usePreviewRouteParams";
export default function Route() {
  const { desktopId, projectId, workspaceId, tabId } = usePreviewRouteParams();
  return <WorkspaceReviewScreen key={workspaceId} desktopId={desktopId} projectId={projectId} workspaceId={workspaceId} returnTabId={tabId} />;
}
