// Verify the built site without JavaScript: routes, headings, internal links, anchors.
// Run the production server first, then: node scripts/check-site.mjs http://127.0.0.1:3101
const origin = process.argv[2] ?? "http://127.0.0.1:3101";
const sitemap = await fetch(`${origin}/sitemap.xml`).then((r) => r.text());
const paths = [...sitemap.matchAll(/<loc>(.*?)<\/loc>/g)].map(
  (m) => new URL(m[1]).pathname,
);
if (paths.length < 40) throw new Error("Sitemap is missing expected pages.");
const pages = new Map();
const failures = [];
for (const path of paths) {
  const response = await fetch(origin + path);
  const html = await response.text();
  if (!response.ok) failures.push(`${path}: HTTP ${response.status}`);
  if ((html.match(/<h1\b/g) ?? []).length !== 1)
    failures.push(`${path}: expected exactly one H1`);
  const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
  if (new Set(ids).size !== ids.length)
    failures.push(`${path}: duplicate element IDs`);
  pages.set(path, { html, ids: new Set(ids) });
}
let links = 0;
for (const [path, page] of pages) {
  for (const match of page.html.matchAll(/<a\b[^>]*\bhref="([^"]+)"/g)) {
    const href = match[1].replaceAll("&amp;", "&");
    const url = new URL(href, origin + path);
    if (![new URL(origin).origin, "https://dragent.dev"].includes(url.origin))
      continue;
    const targetPath = url.pathname.replace(/\/$/, "") || "/";
    const target = pages.get(targetPath);
    links++;
    if (!target) {
      if (/\.(webp|png|jpg|mp4|svg)$/.test(targetPath)) {
        const asset = await fetch(origin + targetPath);
        if (!asset.ok) failures.push(`${path}: missing asset ${href}`);
      } else failures.push(`${path}: missing internal page ${href}`);
    } else if (
      url.hash &&
      !target.ids.has(decodeURIComponent(url.hash.slice(1)))
    )
      failures.push(`${path}: missing anchor ${href}`);
  }
}
const missing = await fetch(origin + "/docs/not-a-real-guide");
if (missing.status !== 404)
  failures.push(`Unknown guide returned ${missing.status}, expected 404`);
console.log(
  JSON.stringify(
    { pages: pages.size, internalLinks: links, failures },
    null,
    2,
  ),
);
if (failures.length) process.exitCode = 1;
