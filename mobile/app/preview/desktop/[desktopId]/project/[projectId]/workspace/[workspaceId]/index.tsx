import { WorkspaceTabsScreen } from "../../../../../../../../src/features/preview/WorkspaceTabsScreen";
import { usePreviewRouteParams } from "../../../../../../../../src/preview/usePreviewRouteParams";

export default function PreviewRoute() {
  const { desktopId, projectId, workspaceId } = usePreviewRouteParams();
  return <WorkspaceTabsScreen key={`${desktopId}/${projectId}/${workspaceId}`} desktopId={desktopId} projectId={projectId} workspaceId={workspaceId} />;
}
