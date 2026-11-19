import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { BUNDLED_THEMES } from "../src/theme/catalog.generated";
import {
  CURATED_THEME_IDS,
  DARK_THEME_ID,
  DEFAULT_THEME_ID,
  LIGHT_THEME_ID,
  accentTextFor,
  blendHex,
  contrastRatio,
  displayNameForId,
  hexToRgb,
  statusMarkColors,
  STATUS_MARK_CONTRAST,
  isDarkBackground,
  normalizeHex,
  normalizeSelection,
  parseDesktopYaml,
  resolveAppTheme,
  toAppTheme,
} from "../src/theme/derive";
import { loadCuratedThemes } from "./sync-desktop-themes";

const VALID_DARK_YAML = `accent: "#53BDFA"
background: "#0d1117"
details: darker
foreground: "#c9d1d9"
terminal_colors:
  normal:
    black: "#0d1117"
    red: "#ff7b72"
    green: "#3fb950"
    yellow: "#d29922"
    blue: "#58a6ff"
    magenta: "#bc8cff"
    cyan: "#76e3ea"
    white: "#b1bac4"
  bright:
    black: "#8b949e"
    red: "#ffa198"
    green: "#56d364"
    yellow: "#e3b341"
    blue: "#79c0ff"
    magenta: "#d2a8ff"
    cyan: "#b3f0ff"
    white: "#b1bac4"
`;

function desktopYaml(name: "gruvbox_dark.yaml" | "gruvbox_light.yaml"): string {
  return readFileSync(
    new URL(`../../dragent/crates/dragent_viewer/assets/themes/standard/${name}`, import.meta.url),
    "utf8",
  );
}

test("YAML parser accepts a canonical desktop theme", () => {
  const palette = parseDesktopYaml(VALID_DARK_YAML, { id: "probe" });
  assert.equal(palette.id, "probe");
  assert.equal(palette.mode, "dark");
  assert.equal(palette.background, "#0d1117");
  assert.equal(palette.foreground, "#c9d1d9");
  assert.equal(palette.accent, "#53bdfa");
  assert.equal(palette.ansiNormal.length, 8);
  assert.equal(palette.ansiBright.length, 8);
});

test("YAML parser tolerates desktop extras and caller names", () => {
  const withImage = `${VALID_DARK_YAML}background_image:\n  path: warp_bundled/koi_bg.jpg\n  opacity: 50\n`;
  const palette = parseDesktopYaml(withImage, { id: "probe", name: "  Probe  " });
  assert.equal(palette.name, "Probe");

  const customDetails = VALID_DARK_YAML.replace("details: darker", "details:\n  custom:\n    opacity: 10\n");
  assert.equal(parseDesktopYaml(customDetails, { id: "probe" }).mode, "dark");

  const noDetails = VALID_DARK_YAML.replace("details: darker\n", "");
  assert.equal(parseDesktopYaml(noDetails, { id: "probe" }).mode, "dark");
});

test("YAML parser averages gradient backgrounds like the desktop", () => {
  const gradient = VALID_DARK_YAML.replace(
    'background: "#0d1117"',
    "background:\n  top: \"#0c252d\"\n  bottom: \"#0c2c35\"",
  );
  assert.equal(parseDesktopYaml(gradient, { id: "probe" }).background, "#0c2931");
});

test("YAML parser fails clearly on malformed palettes", () => {
  assert.throws(() => parseDesktopYaml("", { id: "probe" }), /empty/);
  assert.throws(() => parseDesktopYaml("accent: [", { id: "probe" }), /not valid YAML/);
  assert.throws(() => parseDesktopYaml("accent: 1", { id: "probe" }), /"accent"/);
  assert.throws(
    () => parseDesktopYaml(VALID_DARK_YAML.replace('accent: "#53BDFA"', 'accent: "nope"'), { id: "probe" }),
    /"accent"/,
  );
  assert.throws(
    () => parseDesktopYaml(VALID_DARK_YAML.replace('foreground: "#c9d1d9"\n', ""), { id: "probe" }),
    /"foreground"/,
  );
  assert.throws(
    () => parseDesktopYaml(VALID_DARK_YAML.replace('background: "#0d1117"\n', ""), { id: "probe" }),
    /"background"/,
  );
  assert.throws(
    () => parseDesktopYaml(VALID_DARK_YAML.replace("terminal_colors:", "terminal_colours:"), { id: "probe" }),
    /terminal_colors/,
  );
  assert.throws(
    () => parseDesktopYaml(VALID_DARK_YAML.replace('red: "#ff7b72"', 'red: "#gggggg"'), { id: "probe" }),
    /terminal_colors\.normal\.red/,
  );
});

test("hex normalization matches desktop leniency", () => {
  assert.equal(normalizeHex("#53BDFA"), "#53bdfa");
  assert.equal(normalizeHex("53bdfa"), "#53bdfa");
  assert.equal(normalizeHex("#abc"), "#aabbcc");
  assert.equal(normalizeHex("#0d1117ff"), "#0d1117");
  assert.throws(() => normalizeHex("nope"), /Invalid color/);
  assert.throws(() => normalizeHex(42), /Invalid color/);
});

test("light/dark derivation follows details then luminance", () => {
  assert.equal(isDarkBackground("#111113"), true);
  assert.equal(isDarkBackground("#fafafa"), false);
  assert.equal(displayNameForId("gruvbox_dark"), "Gruvbox Dark");
  assert.equal(displayNameForId("gruvbox_light"), "Gruvbox Light");
  assert.equal(displayNameForId("dark"), "Dark");
  assert.equal(displayNameForId("light"), "Light");

  const lightYaml = VALID_DARK_YAML.replace('background: "#0d1117"', 'background: "#fbf1c7"')
    .replace("details: darker", "details: lighter")
    .replace('foreground: "#c9d1d9"', 'foreground: "#3c3836"');
  assert.equal(parseDesktopYaml(lightYaml, { id: "probe" }).mode, "light");
});

test("curated catalog holds exactly the four offered themes in order", () => {
  assert.deepEqual([...CURATED_THEME_IDS], ["dark", "gruvbox_dark", "gruvbox_light", "light"]);
  assert.equal(BUNDLED_THEMES.length, 4);
  assert.deepEqual(BUNDLED_THEMES.map((entry) => entry.id), [...CURATED_THEME_IDS]);
  assert.deepEqual(BUNDLED_THEMES.map((entry) => entry.name), ["Dark", "Gruvbox Dark", "Gruvbox Light", "Light"]);
  assert.deepEqual(BUNDLED_THEMES.map((entry) => entry.mode), ["dark", "dark", "light", "light"]);
  assert.equal(DEFAULT_THEME_ID, DARK_THEME_ID);
});

test("neutral Dark uses the restrained graphite UI with a monochrome primary", () => {
  const dark = BUNDLED_THEMES[0]!;
  assert.equal(dark.id, "dark");
  assert.deepEqual(dark.colors, {
    background: "#111113",
    surface: "#171719",
    surfaceRaised: "#202023",
    text: "#eeeeef",
    textMuted: "#acacad",
    textSubtle: "#808081",
    border: "#2b2b2f",
    accent: "#eeeeef",
    onAccent: "#0f1216",
    success: "#8fce9e",
    warning: "#d9a441",
    danger: "#e08a8a",
    selection: "#39393b",
  });
  // Muted/subtle/selection are graphite blends of the same pair — no hue shift.
  assert.equal(dark.colors.textMuted, blendHex("#111113", "#eeeeef", 0.7));
  assert.equal(dark.colors.textSubtle, blendHex("#111113", "#eeeeef", 0.5));
  assert.equal(dark.colors.selection, blendHex("#111113", "#eeeeef", 0.18));
  // Monochrome primary: accent carries no blue/cyan/purple tint.
  assert.equal(dark.colors.accent, dark.colors.text);
  assert.equal(dark.colors.onAccent, accentTextFor(dark.colors.accent));
  assert.deepEqual(dark.terminal, {
    background: "#111113",
    foreground: "#eeeeef",
    cursor: "#eeeeef",
    ansiNormal: ["#616161", "#ff8272", "#b4fa72", "#fefdc2", "#a5d5fe", "#ff8ffd", "#d0d1fe", "#f1f1f1"],
    ansiBright: ["#8e8e8e", "#ffc4bd", "#d6fcb9", "#fefdd5", "#c1e3fe", "#ffb1fe", "#e5e6fe", "#feffff"],
  });
});

test("neutral Light uses the near-white UI with a monochrome primary", () => {
  const light = BUNDLED_THEMES[3]!;
  assert.equal(light.id, "light");
  assert.deepEqual(light.colors, {
    background: "#fafafa",
    surface: "#ffffff",
    surfaceRaised: "#f4f4f5",
    text: "#18181b",
    textMuted: "#5c5c5e",
    textSubtle: "#89898b",
    border: "#e4e4e7",
    accent: "#18181b",
    onAccent: "#ffffff",
    success: "#1f7a4d",
    warning: "#8a5a00",
    danger: "#c03535",
    selection: "#d1d1d2",
  });
  assert.equal(light.colors.textMuted, blendHex("#fafafa", "#18181b", 0.7));
  assert.equal(light.colors.textSubtle, blendHex("#fafafa", "#18181b", 0.5));
  assert.equal(light.colors.selection, blendHex("#fafafa", "#18181b", 0.18));
  assert.equal(light.colors.accent, light.colors.text);
  assert.equal(light.colors.onAccent, accentTextFor(light.colors.accent));
  assert.deepEqual(light.terminal, {
    background: "#fafafa",
    foreground: "#18181b",
    cursor: "#18181b",
    ansiNormal: ["#1c1c1e", "#c4362a", "#348c48", "#c48412", "#246ec4", "#a03b8f", "#2a8f88", "#cacaca"],
    ansiBright: ["#636363", "#d64939", "#469e5a", "#d69624", "#3680d6", "#b24da1", "#3ca19a", "#ffffff"],
  });
});

test("Gruvbox entries come from the canonical desktop YAML, not hand copies", () => {
  const darkRaw = desktopYaml("gruvbox_dark.yaml");
  const lightRaw = desktopYaml("gruvbox_light.yaml");
  const expectedDark = toAppTheme(parseDesktopYaml(darkRaw, { id: "gruvbox_dark", name: "Gruvbox Dark" }));
  const expectedLight = toAppTheme(parseDesktopYaml(lightRaw, { id: "gruvbox_light", name: "Gruvbox Light" }));
  assert.deepEqual(BUNDLED_THEMES[1], expectedDark);
  assert.deepEqual(BUNDLED_THEMES[2], expectedLight);

  // Spot checks guard against silent source drift.
  assert.equal(expectedDark.colors.background, "#282828");
  assert.equal(expectedDark.colors.accent, "#56c2b8");
  assert.equal(expectedDark.mode, "dark");
  assert.equal(expectedLight.colors.background, "#fbf1c7");
  assert.equal(expectedLight.mode, "light");
  // Status follows the desktop palette (error=red, ok=green, warn=yellow).
  for (const entry of [expectedDark, expectedLight]) {
    assert.equal(entry.colors.success, entry.terminal.ansiNormal[2]);
    assert.equal(entry.colors.warning, entry.terminal.ansiNormal[3]);
    assert.equal(entry.colors.danger, entry.terminal.ansiNormal[1]);
  }
});

test("generated catalog matches a fresh curated build", () => {
  assert.deepEqual(BUNDLED_THEMES, loadCuratedThemes());
});

test("every curated theme keeps text and on-accent readable", () => {
  for (const theme of BUNDLED_THEMES) {
    assert.ok(
      contrastRatio(theme.colors.text, theme.colors.background) >= 4.5,
      `${theme.id}: text contrast`,
    );
    // Small labels must meet AA in all four themes.
    assert.equal(theme.colors.onAccent, accentTextFor(theme.colors.accent), `${theme.id}: onAccent pole`);
    assert.ok(
      contrastRatio(theme.colors.onAccent, theme.colors.accent) >= 4.5,
      `${theme.id}: onAccent contrast`,
    );
  }
  // The neutral pair clears full AA on its monochrome primary.
  for (const id of ["dark", "light"] as const) {
    const theme = BUNDLED_THEMES.find((entry) => entry.id === id)!;
    assert.ok(contrastRatio(theme.colors.onAccent, theme.colors.accent) >= 4.5, `${id}: onAccent AA`);
  }
});

test("neutral muted, subtle, and status colors stay accessible", () => {
  for (const id of ["dark", "light"] as const) {
    const theme = BUNDLED_THEMES.find((entry) => entry.id === id)!;
    assert.ok(contrastRatio(theme.colors.textMuted, theme.colors.background) >= 4.5, `${id}: muted`);
    assert.ok(contrastRatio(theme.colors.textSubtle, theme.colors.background) >= 3, `${id}: subtle`);
    for (const key of ["success", "warning", "danger"] as const) {
      assert.ok(contrastRatio(theme.colors[key], theme.colors.background) >= 4.5, `${id}: ${key}`);
    }
  }
});

test("selection resolves curated ids and normalizes every legacy value to Dark", () => {
  const themes = [...BUNDLED_THEMES];
  assert.equal(resolveAppTheme("dark", "light", themes).id, "dark");
  assert.equal(resolveAppTheme("gruvbox_dark", "light", themes).id, "gruvbox_dark");
  assert.equal(resolveAppTheme("gruvbox_light", "dark", themes).id, "gruvbox_light");

  // Legacy `system` follows the OS without adding a fifth public theme.
  assert.equal(resolveAppTheme("system", "light", themes).id, "light");
  assert.equal(resolveAppTheme("system", "dark", themes).id, "dark");
  assert.equal(resolveAppTheme("system", null, themes).id, "dark");
  assert.deepEqual(themes.map((entry) => entry.id), [...CURATED_THEME_IDS]);

  // Retired and unknown ids collapse to the stable Dark default.
  for (const legacy of ["github_dark", "custom", "dracula", "missing-theme", ""]) {
    assert.equal(resolveAppTheme(legacy, "light", themes).id, DARK_THEME_ID, legacy || "(empty)");
  }
});

test("normalizeSelection keeps curated ids and maps legacy to Dark", () => {
  for (const id of CURATED_THEME_IDS) {
    assert.equal(normalizeSelection(id, "light"), id);
    assert.equal(normalizeSelection(id, "dark"), id);
  }
  assert.equal(normalizeSelection("system", "light"), LIGHT_THEME_ID);
  assert.equal(normalizeSelection("system", "dark"), DARK_THEME_ID);
  assert.equal(normalizeSelection("system", null), DARK_THEME_ID);
  assert.equal(normalizeSelection("system", undefined), DARK_THEME_ID);
  for (const legacy of ["github_dark", "custom", "dracula", "typo", ""]) {
    assert.equal(normalizeSelection(legacy, "light"), DARK_THEME_ID, legacy || "(empty)");
  }
});

test("status marks are sharp on every theme, in each theme's own colour family", () => {
  const hue = (hex: string): number => {
    const { r, g, b } = hexToRgb(hex);
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    if (max === min) return 0;
    const d = max - min;
    const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    return (h * 60 + 360) % 360;
  };
  // A desktop-derived theme can bring a pale palette; it must stay sharp too.
  const pale = { background: "#ffffff", success: "#b8f5c8", warning: "#ffe680", danger: "#ffb3b3", textMuted: "#cccccc" };
  for (const colors of [...BUNDLED_THEMES.map((theme) => theme.colors), pale]) {
    const marks = statusMarkColors(colors);
    for (const key of ["success", "warning", "danger"] as const) {
      assert.ok(
        contrastRatio(marks[key], colors.background) >= STATUS_MARK_CONTRAST,
        `${key} ${marks[key]} on ${colors.background}`,
      );
      const drift = Math.abs(hue(marks[key]) - hue(colors[key]));
      assert.ok(Math.min(drift, 360 - drift) <= 6, `${key} kept its family: ${colors[key]} → ${marks[key]}`);
    }
  }
  // Already sharp: untouched.
  const light = BUNDLED_THEMES.find((theme) => theme.id === "light")!.colors;
  assert.equal(statusMarkColors(light).success, light.success);
});
