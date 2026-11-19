/**
 * A coordination's goal as the lead reads it: what the person typed, then
 * each linked issue the way the desktop's prompt links them (a heading with
 * the tracker's key and title, then its address), then attached files.
 */
import { withAttachments } from "../chat/extras";

/** The desktop's cap on a goal. */
export const MAX_GOAL_CHARS = 8000;

export interface LinkedIssue {
  /** `provider:id`, unique across trackers. */
  readonly ref: string;
  readonly provider: string;
  readonly trackerLabel: string;
  readonly key: string;
  readonly title: string;
  readonly url: string | null;
}

export function issueRef(provider: string, id: string): string {
  return `${provider}:${id}`;
}

export function goalWithLinks(text: string, issues: readonly LinkedIssue[], files: readonly string[]): string {
  const parts: string[] = [];
  const typed = text.trim();
  if (typed) parts.push(typed);
  else if (issues.length > 0) parts.push("Work on the linked issues below. Use their details as context.");
  for (const issue of issues) {
    const name = issue.key ? `${issue.key}: ${issue.title}` : issue.title;
    parts.push(issue.url ? `## Linked issue — ${name}\n${issue.url}` : `## Linked issue — ${name}`);
  }
  return withAttachments(parts.join("\n\n"), files);
}
