/**
 * A small reader for the Markdown the agent writes: paragraphs, headings,
 * lists, quotes, rules and fenced code, with bold, italic, code and links
 * inline. Pure text in, plain data out; unknown syntax stays readable text.
 */

export type MarkdownInline =
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "bold"; readonly text: string }
  | { readonly kind: "italic"; readonly text: string }
  | { readonly kind: "code"; readonly text: string }
  | { readonly kind: "link"; readonly text: string; readonly href: string };

export type MarkdownBlock =
  | { readonly kind: "paragraph"; readonly inline: readonly MarkdownInline[] }
  | { readonly kind: "heading"; readonly level: number; readonly inline: readonly MarkdownInline[] }
  | { readonly kind: "item"; readonly marker: string; readonly depth: number; readonly inline: readonly MarkdownInline[] }
  | { readonly kind: "quote"; readonly inline: readonly MarkdownInline[] }
  | { readonly kind: "code"; readonly language: string; readonly text: string }
  | { readonly kind: "rule" };

const INLINE = /(`[^`\n]+`)|(\*\*[^*\n]+\*\*)|(__[^_\n]+__)|(\[[^\]\n]+\]\((?:https?:\/\/)[^)\s]+\))|(\*[^*\s][^*\n]*\*)/gu;

/** Inline runs of one line or paragraph. */
export function markdownInline(source: string): readonly MarkdownInline[] {
  const out: MarkdownInline[] = [];
  let last = 0;
  for (const match of source.matchAll(INLINE)) {
    const start = match.index ?? 0;
    if (start > last) out.push({ kind: "text", text: source.slice(last, start) });
    const token = match[0];
    if (match[1]) out.push({ kind: "code", text: token.slice(1, -1) });
    else if (match[2] || match[3]) out.push({ kind: "bold", text: token.slice(2, -2) });
    else if (match[4]) {
      const split = token.indexOf("](");
      out.push({ kind: "link", text: token.slice(1, split), href: token.slice(split + 2, -1) });
    } else out.push({ kind: "italic", text: token.slice(1, -1) });
    last = start + token.length;
  }
  if (last < source.length) out.push({ kind: "text", text: source.slice(last) });
  return out;
}

/** The blocks of a Markdown text, in order. A fence left open (streaming) is still code. */
export function markdownBlocks(source: string): readonly MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  const lines = source.replace(/\r\n?/gu, "\n").split("\n");
  let paragraph: string[] = [];
  const flush = () => {
    if (paragraph.length === 0) return;
    blocks.push({ kind: "paragraph", inline: markdownInline(paragraph.join("\n")) });
    paragraph = [];
  };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]!;
    const fence = /^\s*(```|~~~)\s*([\w+#.-]*)\s*$/u.exec(line);
    if (fence) {
      flush();
      const body: string[] = [];
      index += 1;
      while (index < lines.length && !/^\s*(```|~~~)\s*$/u.test(lines[index]!)) {
        body.push(lines[index]!);
        index += 1;
      }
      blocks.push({ kind: "code", language: fence[2] ?? "", text: body.join("\n") });
      continue;
    }
    if (line.trim() === "") {
      flush();
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/u.exec(line);
    if (heading) {
      flush();
      blocks.push({ kind: "heading", level: heading[1]!.length, inline: markdownInline(heading[2]!.replace(/\s+#+\s*$/u, "")) });
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/u.test(line)) {
      flush();
      blocks.push({ kind: "rule" });
      continue;
    }
    const item = /^(\s*)([-*+]|\d{1,3}[.)])\s+(.*)$/u.exec(line);
    if (item) {
      flush();
      const bullet = item[2]!;
      blocks.push({
        kind: "item",
        marker: /^\d/u.test(bullet) ? bullet.replace(")", ".") : "•",
        depth: Math.min(4, Math.floor(item[1]!.replace(/\t/gu, "  ").length / 2)),
        inline: markdownInline(item[3]!),
      });
      continue;
    }
    const quote = /^\s*>\s?(.*)$/u.exec(line);
    if (quote) {
      flush();
      blocks.push({ kind: "quote", inline: markdownInline(quote[1]!) });
      continue;
    }
    paragraph.push(line);
  }
  flush();
  return blocks;
}
