import { WorkspaceTabScreen } from "../../../../../../../../../src/features/workspaces/WorkspaceTabScreen";
import { usePreviewRouteParams } from "../../../../../../../../../src/preview/usePreviewRouteParams";

export default function PreviewRoute() {
  const { desktopId, projectId, workspaceId, tabId } = usePreviewRouteParams();
  return <WorkspaceTabScreen key={`${desktopId}/${projectId}/${workspaceId}`} desktopId={desktopId} projectId={projectId} workspaceId={workspaceId} tabId={tabId} />;
}
