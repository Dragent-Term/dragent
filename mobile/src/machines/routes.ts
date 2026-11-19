/**
 * Live machine route helpers. Catalog IDs may contain slashes (stable
 * repository/worktree paths are allowed IDs) and even literal `%` sequences.
 *
 * Linking and hooks may both percent-decode a segment. A versioned escape
 * alphabet carries complex IDs without percent signs, preserving literal
 * `%2F` independently of `/` through either decoder. Simple UUID/name IDs
 * keep their existing URLs. Browser QA exercises the full router pipeline.
 */

function routeSegment(id: string): string {
  if (!id || /^[A-Za-z0-9._-]+$/.test(id)) return id;
  return "r1~" + encodeURIComponent(id)
    .replace(/[!'()*~]/g, (char) => "%" + char.charCodeAt(0).toString(16).toUpperCase())
    .replace(/%/g, "~");
}

/** Single-valued router param; repeated or missing identifiers select nothing. */
export function singleParam(value: string | string[] | undefined): string {
  if (typeof value !== "string") return "";
  if (!value.startsWith("r1~")) return value;
  try { return decodeURIComponent(value.slice(3).replace(/~/g, "%")); }
  catch { return ""; }
}

export function machinePath(deviceId: string): {
  pathname: "/machine/[deviceId]";
  params: { deviceId: string };
} {
  return {
    pathname: "/machine/[deviceId]",
    params: { deviceId: routeSegment(deviceId) },
  };
}

/** The desktop's workspace list, filtered to `show` (`needs_you`, `review`…). */
export function machineWorkspacesShowing(deviceId: string, show: string): {
  pathname: "/machine/[deviceId]";
  params: { deviceId: string; view: string; show: string };
} {
  return {
    pathname: "/machine/[deviceId]",
    params: { deviceId: routeSegment(deviceId), view: "projects", show },
  };
}

export function machineProjectPath(
  deviceId: string,
  projectKey: string,
): {
  pathname: "/machine/[deviceId]/project/[projectId]";
  params: { deviceId: string; projectId: string };
} {
  return {
    pathname: "/machine/[deviceId]/project/[projectId]",
    params: {
      deviceId: routeSegment(deviceId),
      projectId: routeSegment(projectKey),
    },
  };
}

export function machineWorkspacePath(
  deviceId: string,
  workspaceKey: string,
): {
  pathname: "/machine/[deviceId]/workspace/[workspaceId]";
  params: { deviceId: string; workspaceId: string };
} {
  return {
    pathname: "/machine/[deviceId]/workspace/[workspaceId]",
    params: {
      deviceId: routeSegment(deviceId),
      workspaceId: routeSegment(workspaceKey),
    },
  };
}

export function machineTabPath(
  deviceId: string,
  tabId: number,
): {
  pathname: "/machine/[deviceId]/tab/[tabId]";
  params: { deviceId: string; tabId: string };
} {
  return {
    pathname: "/machine/[deviceId]/tab/[tabId]",
    params: { deviceId: routeSegment(deviceId), tabId: String(tabId) },
  };
}

export function machineCoordinationDetailPath(
  deviceId: string,
  teamId: string,
): {
  pathname: "/machine/[deviceId]/coordination/[coordinationId]";
  params: { deviceId: string; coordinationId: string };
} {
  return {
    pathname: "/machine/[deviceId]/coordination/[coordinationId]",
    params: {
      deviceId: routeSegment(deviceId),
      coordinationId: routeSegment(teamId),
    },
  };
}

export function machineTaskPath(
  deviceId: string,
  task: string,
): {
  pathname: "/machine/[deviceId]/task/[taskId]";
  params: { deviceId: string; taskId: string };
} {
  return {
    pathname: "/machine/[deviceId]/task/[taskId]",
    params: { deviceId: routeSegment(deviceId), taskId: routeSegment(task) },
  };
}

export function machineNewTaskPath(deviceId: string): {
  pathname: "/machine/[deviceId]/new-task";
  params: { deviceId: string };
} {
  return {
    pathname: "/machine/[deviceId]/new-task",
    params: { deviceId: routeSegment(deviceId) },
  };
}

/** Allow or Deny from a notification, answered through the desktop. */
export function machineAnswerPath(
  deviceId: string,
  pane: string,
  choice: "allow" | "deny",
): {
  pathname: "/machine/[deviceId]/answer";
  params: { deviceId: string; pane: string; choice: "allow" | "deny" };
} {
  return {
    pathname: "/machine/[deviceId]/answer",
    params: { deviceId: routeSegment(deviceId), pane, choice },
  };
}

/** Past sessions in a desktop folder (a project or a workspace). */
export function machineHistoryPath(
  deviceId: string,
  folder: string,
  title: string,
): {
  pathname: "/machine/[deviceId]/history";
  params: { deviceId: string; folder: string; title: string };
} {
  return {
    pathname: "/machine/[deviceId]/history",
    params: { deviceId: routeSegment(deviceId), folder, title },
  };
}

/** A new agent session; `project` and `workspace` (desktop folders) preselect them. */
export function machineNewSessionPath(
  deviceId: string,
  choose: { project?: string; workspace?: string } = {},
): {
  pathname: "/machine/[deviceId]/new-session";
  params: { deviceId: string; project?: string; workspace?: string };
} {
  return {
    pathname: "/machine/[deviceId]/new-session",
    params: {
      deviceId: routeSegment(deviceId),
      ...(choose.project ? { project: choose.project } : {}),
      ...(choose.workspace ? { workspace: choose.workspace } : {}),
    },
  };
}

export function machineNewCoordinationPath(deviceId: string): {
  pathname: "/machine/[deviceId]/new-coordination";
  params: { deviceId: string };
} {
  return {
    pathname: "/machine/[deviceId]/new-coordination",
    params: { deviceId: routeSegment(deviceId) },
  };
}

export function machineAccountsPath(deviceId: string): {
  pathname: "/machine/[deviceId]/accounts";
  params: { deviceId: string };
} {
  return {
    pathname: "/machine/[deviceId]/accounts",
    params: { deviceId: routeSegment(deviceId) },
  };
}
