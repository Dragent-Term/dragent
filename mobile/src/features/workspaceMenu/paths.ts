/** Path arithmetic for the phone's file browser. Absolute POSIX paths only. */

export function joinPath(folder: string, name: string): string {
  return folder === "/" ? `/${name}` : `${folder}/${name}`;
}

export function parentPath(path: string): string {
  const cut = path.lastIndexOf("/");
  return cut <= 0 ? "/" : path.slice(0, cut);
}

export function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1) || path;
}

/** `path` relative to `root`, or null when it is outside it. "" is the root itself. */
export function relativeTo(root: string, path: string): string | null {
  if (path === root) return "";
  return path.startsWith(`${root}/`) ? path.slice(root.length + 1) : null;
}

/** The folders from the root down to `path`, for breadcrumbs. */
export function crumbs(root: string, path: string): { readonly label: string; readonly path: string }[] {
  const out = [{ label: baseName(root), path: root }];
  const relative = relativeTo(root, path);
  if (!relative) return out;
  let current = root;
  for (const part of relative.split("/")) {
    current = joinPath(current, part);
    out.push({ label: part, path: current });
  }
  return out;
}

/** Folders first, then names, ignoring case (as the desktop's tree sorts). */
export function sortEntries<T extends { readonly name: string; readonly kind: string }>(entries: readonly T[]): T[] {
  return [...entries].sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === "directory" ? -1 : 1;
    return a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });
  });
}

/** "12.4 KB"; bytes below 1 KB as "640 B". */
export function sizeLabel(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
