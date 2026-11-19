/**
 * A code review being written on the phone: comments on diff lines, kept
 * per workspace until they're sent to an agent (or cleared). In memory only.
 */
import { useEffect, useState } from "react";

export interface ReviewComment {
  readonly id: string;
  readonly file: string;
  /** Line in the new version (or the old one for a removed line). */
  readonly line: number | null;
  readonly side: "new" | "old";
  /** The line commented on, as it reads. */
  readonly quote: string;
  readonly text: string;
}

const reviews = new Map<string, ReviewComment[]>();
const listeners = new Map<string, Set<(comments: readonly ReviewComment[]) => void>>();
let next = 0;

function publish(root: string): void {
  const list = [...(reviews.get(root) ?? [])];
  for (const listener of listeners.get(root) ?? []) listener(list);
}

export function addReviewComment(root: string, comment: Omit<ReviewComment, "id">): void {
  next += 1;
  reviews.set(root, [...(reviews.get(root) ?? []), { ...comment, id: `c${next}` }]);
  publish(root);
}

export function removeReviewComment(root: string, id: string): void {
  reviews.set(root, (reviews.get(root) ?? []).filter((comment) => comment.id !== id));
  publish(root);
}

export function clearReview(root: string): void {
  reviews.delete(root);
  publish(root);
}

export function reviewComments(root: string): readonly ReviewComment[] {
  return reviews.get(root) ?? [];
}

export function useReview(root: string): readonly ReviewComment[] {
  const [comments, setComments] = useState<readonly ReviewComment[]>(() => reviewComments(root));
  useEffect(() => {
    setComments(reviewComments(root));
    const set = listeners.get(root) ?? new Set();
    set.add(setComments);
    listeners.set(root, set);
    return () => {
      set.delete(setComments);
    };
  }, [root]);
  return comments;
}

/**
 * The review as one message for an agent: each comment with its file, line
 * and the line it's about, grouped by file, then an optional summary.
 */
export function reviewMessage(comments: readonly ReviewComment[], summary = ""): string {
  const byFile = new Map<string, ReviewComment[]>();
  for (const comment of comments) byFile.set(comment.file, [...(byFile.get(comment.file) ?? []), comment]);
  const parts: string[] = ["Please address these review comments on the current changes:"];
  for (const [file, list] of byFile) {
    parts.push("", `${file}:`);
    for (const comment of list) {
      const where = comment.line === null ? "" : `line ${comment.line}${comment.side === "old" ? " (removed)" : ""}: `;
      const quote = comment.quote.trim() ? ` \`${comment.quote.trim().slice(0, 200)}\`` : "";
      parts.push(`- ${where}${comment.text.trim()}${quote ? ` —${quote}` : ""}`);
    }
  }
  if (summary.trim()) parts.push("", summary.trim());
  return parts.join("\n");
}
