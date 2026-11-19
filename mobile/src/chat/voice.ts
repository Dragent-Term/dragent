/**
 * Voice mode's words: what an agent's reply sounds like read aloud. Code,
 * tables and links don't read well, so they're left in the chat and named.
 */

/** About a minute of speech; the rest stays in the chat. */
export const SPOKEN_MAX = 900;

export function spokenText(markdown: string, max = SPOKEN_MAX): string {
  let text = markdown.replace(/\r\n/g, "\n");
  let hadCode = false;
  text = text.replace(/```[\s\S]*?(```|$)/g, () => {
    hadCode = true;
    return " ";
  });
  text = text
    .replace(/`([^`]+)`/g, "$1")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/https?:\/\/\S+/g, "a link")
    // Line starts only ([ \t], not \s: a blank line between paragraphs must stay).
    .replace(/^[ \t]{0,3}#{1,6}[ \t]+/gm, "")
    .replace(/^[ \t]*[-*+][ \t]+/gm, "")
    .replace(/^[ \t]*\d+\.[ \t]+/gm, "")
    .replace(/^[ \t]*>[ \t]?/gm, "")
    .replace(/^[ \t]*\|.*\|[ \t]*$/gm, "")
    .replace(/[*_~]{1,3}([^*_~]+)[*_~]{1,3}/g, "$1")
    .replace(/\n{2,}/g, ". ")
    .replace(/\n/g, " ")
    .replace(/\s{2,}/g, " ")
    .replace(/\.\s*\./g, ".")
    .trim();
  if (text.length > max) {
    const cut = text.slice(0, max);
    const end = Math.max(cut.lastIndexOf(". "), cut.lastIndexOf("? "), cut.lastIndexOf("! "));
    text = `${(end > max * 0.5 ? cut.slice(0, end + 1) : cut).trim()} The rest is in the chat.`;
  }
  if (hadCode) text = `${text}${text ? " " : ""}I've left the code in the chat.`.trim();
  return text || "Done. It's in the chat.";
}
