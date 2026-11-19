// Crop native 3200×1800 preview captures at intentional feature boundaries.
// Run: node scripts/prepare-product-screens.mjs /path/to/ui-preview
import sharp from "sharp";
import path from "node:path";
const source = process.argv[2];
if (!source) throw new Error("Pass the native UI capture directory.");
for (const [name, scene] of Object.entries({
  coordination: "teams_dark",
  reviews: "pr_files",
  checks: "pr_checks",
  remote: "demo_ssh_machines",
  automations: "demo_automations",
  agents: "agent_transcript",
  editor: "demo_editor",
  workspace: "demo_project_solo",
  integrations: "integrations_dark",
  accounts: "demo_accounts",
})) {
  await sharp(path.join(source, scene + ".png"))
    .resize({ width: 1800, withoutEnlargement: true })
    .webp({ quality: 88 })
    .toFile("public/screens/" + name + ".webp");
}
const scale = 3200 / 2048;
for (const [name, file, rect] of [
  ["coordination-focus", "teams_dark", [440, 350, 1190, 730]],
  ["workspace-focus", "demo_project_coordinate", [700, 420, 950, 400]],
  ["review-focus", "pr_files", [435, 165, 1610, 980]],
  ["remote-focus", "demo_ssh_system", [495, 45, 1350, 800]],
  ["automations-focus", "demo_automations", [440, 35, 1170, 540]],
  ["agent-focus", "agent_transcript", [405, 0, 1220, 700]],
  ["editor-focus", "demo_editor", [290, 52, 1750, 1040]],
]) {
  const [left, top, width, height] = rect.map((n) => Math.round(n * scale));
  await sharp(path.join(source, file + ".png"))
    .extract({ left, top, width, height })
    .resize({ width: 1440, withoutEnlargement: true })
    .webp({ quality: 90 })
    .toFile("public/screens/" + name + ".webp");
}
