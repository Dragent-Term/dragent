import { useLocalSearchParams } from "expo-router";

type RouteParams = {
  desktopId?: string | string[];
  projectId?: string | string[];
  workspaceId?: string | string[];
  tabId?: string | string[];
  id?: string | string[];
  coordinationId?: string | string[];
  automationId?: string | string[];
  reviewId?: string | string[];
};

function single(value: string | string[] | undefined): string {
  return typeof value === "string" ? value : "";
}

/** Repeated or missing identifiers cannot silently select another resource. */
export function usePreviewRouteParams() {
  const params = useLocalSearchParams<RouteParams>();
  return {
    desktopId: single(params.desktopId),
    projectId: single(params.projectId),
    workspaceId: single(params.workspaceId),
    tabId: single(params.tabId),
    id: single(params.id),
    coordinationId: single(params.coordinationId),
    automationId: single(params.automationId),
    reviewId: single(params.reviewId),
  };
}
