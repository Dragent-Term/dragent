import { ProjectWorkspacesScreen } from "../../../../../../src/features/preview/ProjectWorkspacesScreen";
import { usePreviewRouteParams } from "../../../../../../src/preview/usePreviewRouteParams";

export default function PreviewRoute() {
  const { desktopId, projectId } = usePreviewRouteParams();
  return <ProjectWorkspacesScreen key={`${desktopId}/${projectId}`} desktopId={desktopId} projectId={projectId} />;
}
