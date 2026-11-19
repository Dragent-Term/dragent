import { Redirect, router } from "expo-router";
import { EmptyState } from "../components";
import { ScrollScreen } from "../layout";
import { usePreview } from "./PreviewContext";
import { selectPreviewTab, selectPreviewWorkspace } from "./PreviewStore";
import { usePreviewRouteParams } from "./usePreviewRouteParams";

function Unavailable({ title }: { title: string }) {
  return <ScrollScreen><EmptyState title={title} message="Return to your preview desktops to continue."
    actionLabel="Desktops" onAction={() => router.replace("/preview")} /></ScrollScreen>;
}

/** Earlier project/tab bookmarks resolve through their actual workspace. */
export function LegacyProjectTabRoute() {
  const { desktopId, projectId, tabId } = usePreviewRouteParams();
  const preview = usePreview();
  const detail = preview.getTab(tabId);
  const tab = detail ? selectPreviewTab(preview, desktopId, projectId, detail.workspaceId, tabId) : null;
  if (!tab) return <Unavailable title="Tab unavailable" />;
  return <Redirect href={{
    pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]/tab/[tabId]",
    params: { desktopId, projectId, workspaceId: tab.workspaceId, tabId },
  }} />;
}

export function LegacyWorkspaceRoute() {
  const { id } = usePreviewRouteParams();
  const preview = usePreview();
  const workspace = preview.getWorkspace(id);
  const project = preview.projects.find((entry) => entry.id === workspace?.projectId);
  if (!project || !workspace || !selectPreviewWorkspace(preview, project.desktopId, project.id, workspace.id)) {
    return <Unavailable title="Workspace unavailable" />;
  }
  return <Redirect href={{
    pathname: "/preview/desktop/[desktopId]/project/[projectId]/workspace/[workspaceId]",
    params: { desktopId: project.desktopId, projectId: project.id, workspaceId: workspace.id },
  }} />;
}
