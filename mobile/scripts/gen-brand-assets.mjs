// Regenerates every mobile brand raster from the master mark in
// ../dragent/brand. Never hand-edit the PNGs in assets/brand.
//   node scripts/gen-brand-assets.mjs
import { Resvg } from "@resvg/resvg-js";
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const brand = join(root, "..", "dragent", "brand");
const out = join(root, "assets");
const read = (name) => readFileSync(join(brand, name), "utf8");

const tile = read("dragent-icon.svg"); // rounded tile + mark
const avatar = read("dragent-avatar.svg"); // full-bleed tile + mark
const defs = tile.match(/<defs>[\s\S]*?<\/defs>/)[0];
const markShapes = tile.split("\n").filter((l) => l.includes("<polygon")).join("\n");
// The mark's visual centre in master coordinates.
const CX = 513, CY = 512;

const svg = (body, size = 1024) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 1024 1024">${body}</svg>`;
const mark = (scale) =>
  `<g transform="translate(512 512) scale(${scale}) translate(${-CX} ${-CY})">${markShapes}</g>`;
const silhouette = (scale, color) =>
  `<g transform="translate(512 512) scale(${scale}) translate(${-CX} ${-CY})" fill="${color}">${
    markShapes.replace(/ fill="[^"]*"/g, "").replace(/ opacity="[^"]*"/g, "")}</g>`;

function png(source, size, file, { opaque } = {}) {
  const r = new Resvg(source, {
    fitTo: { mode: "width", value: size },
    ...(opaque ? { background: "#0a0e15" } : {}),
  });
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, r.render().asPng());
  console.log(file.replace(root + "/", ""), size);
}

// iOS app icon: opaque, full-bleed (the OS applies the corner mask).
png(avatar, 1024, join(out, "icon.png"), { opaque: true });
// App Store rejects an alpha channel on the 1024 icon; macOS-only flatten.
if (process.platform === "darwin") {
  const icon = join(out, "icon.png"), tmp = join(out, ".icon.jpg");
  execFileSync("sips", ["-s", "format", "jpeg", "-s", "formatOptions", "100", icon, "--out", tmp]);
  execFileSync("sips", ["-s", "format", "png", tmp, "--out", icon]);
  execFileSync("rm", [tmp]);
}
// Android adaptive icon: mark inside the 66% safe zone on a transparent layer.
png(svg(defs + mark(0.92)), 1024, join(out, "adaptive-icon.png"));
png(svg(defs + silhouette(0.92, "#ffffff")), 1024, join(out, "adaptive-icon-monochrome.png"));
// Splash: mark only, sized by the splash plugin's imageWidth.
png(svg(defs + mark(1.0)), 1024, join(out, "splash.png"));
// Android status-bar notification glyph (white on transparent).
png(svg(silhouette(1.15, "#ffffff")), 96, join(out, "notification-icon.png"));
// Web favicon / PWA: rounded tile.
png(tile, 196, join(out, "favicon.png"));
