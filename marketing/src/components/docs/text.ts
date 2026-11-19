import { isValidElement, type ReactNode } from "react";
export function nodeText(node: ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join(" ");
  if (
    isValidElement<{
      children?: ReactNode;
      code?: string;
      head?: string[];
      rows?: ReactNode[][];
    }>(node)
  )
    return [
      nodeText(node.props.children),
      node.props.code ?? "",
      nodeText(node.props.head),
      nodeText(node.props.rows),
    ]
      .filter(Boolean)
      .join(" ");
  return "";
}
export function headingId(text: string) {
  return text
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9\s-]/g, "")
    .trim()
    .replace(/\s+/g, "-");
}
