import assert from "node:assert/strict";
import { test } from "node:test";
import {
  AGENT_ALIASES,
  AGENT_ARTWORK_SOURCES,
  SUPPORTED_AGENT_IDS,
  getAgentLabel,
  getAgentThemeVariant,
  normalizeAgentKind,
} from "../src/components/agentArtwork";

test("supported catalog ids are exactly the seven vendored marks", () => {
  assert.deepEqual([...SUPPORTED_AGENT_IDS], [
    "dragent",
    "claude",
    "codex",
    "gemini",
    "opencode",
    "amp",
    "cursor",
  ]);
});

test("live aliases normalize case/whitespace-insensitively", () => {
  assert.equal(normalizeAgentKind("claude"), "claude");
  assert.equal(normalizeAgentKind("claude-code"), "claude");
  assert.equal(normalizeAgentKind("  Claude-Code  "), "claude");
  assert.equal(normalizeAgentKind("CLAUDE"), "claude");
  assert.equal(normalizeAgentKind("amp"), "amp");
  assert.equal(normalizeAgentKind("ampcode"), "amp");
  assert.equal(normalizeAgentKind("AMPCODE"), "amp");
  assert.equal(normalizeAgentKind("cursor"), "cursor");
  assert.equal(normalizeAgentKind("cursor-agent"), "cursor");
  assert.equal(normalizeAgentKind("codex"), "codex");
  assert.equal(normalizeAgentKind("codex-cli"), "codex");
  assert.equal(normalizeAgentKind("gemini"), "gemini");
  assert.equal(normalizeAgentKind("gemini-cli"), "gemini");
  assert.equal(normalizeAgentKind("dragent"), "dragent");
  assert.equal(normalizeAgentKind("opencode"), "opencode");
  // Legacy preview ids stay compatible.
  assert.equal(normalizeAgentKind("dragent"), "dragent");
});

test("unknown and custom kinds never map to a vendor (no OpenCode fallback)", () => {
  for (const unknown of [
    "custom:acme",
    "custom:foo",
    "CUSTOM:Acme",
    "",
    "   ",
    "unknown-agent",
    "opencode-custom",
    "my-claude",
    "claudecodeplus",
  ]) {
    assert.equal(normalizeAgentKind(unknown), null, `expected null for ${JSON.stringify(unknown)}`);
  }
  for (const bad of [null, undefined, 123, {}, [], true]) {
    assert.equal(normalizeAgentKind(bad), null);
  }
  // The old bug rendered everything unknown as OpenCode; guard it here.
  assert.notEqual(normalizeAgentKind("some-brand-new-agent"), "opencode");
  assert.notEqual(normalizeAgentKind("custom:opencode"), "opencode");
});

test("aliases only point at supported ids", () => {
  const supported: ReadonlySet<string> = new Set(SUPPORTED_AGENT_IDS);
  for (const [alias, target] of Object.entries(AGENT_ALIASES)) {
    assert.ok(supported.has(target), `alias ${alias} targets unsupported ${target}`);
    assert.ok(!supported.has(alias) || alias === target, `alias ${alias} shadows a supported id`);
  }
  assert.equal(AGENT_ALIASES["claude-code"], "claude");
  assert.equal(AGENT_ALIASES["ampcode"], "amp");
  assert.equal(AGENT_ALIASES["cursor-agent"], "cursor");
});

test("labels and theme variants follow the native catalog", () => {
  assert.equal(getAgentLabel("claude"), "Claude Code");
  assert.equal(getAgentLabel("gemini"), "Gemini CLI");
  assert.equal(getAgentLabel("cursor"), "Cursor");
  assert.equal(getAgentLabel("amp"), "Amp");
  // Theme variants mirror PromptAgent::image_path(is_dark): cursor/codex/opencode
  // ship light-dark pairs, the rest are single-variant.
  assert.equal(getAgentThemeVariant("cursor"), "light-dark");
  assert.equal(getAgentThemeVariant("codex"), "light-dark");
  assert.equal(getAgentThemeVariant("opencode"), "light-dark");
  assert.equal(getAgentThemeVariant("claude"), "single");
  assert.equal(getAgentThemeVariant("amp"), "single");
  assert.equal(getAgentThemeVariant("gemini"), "single");
  assert.equal(getAgentThemeVariant("dragent"), "single");
});

test("every supported id documents a vendored source file", () => {
  for (const id of SUPPORTED_AGENT_IDS) {
    const source = AGENT_ARTWORK_SOURCES[id];
    assert.ok(source, `missing source for ${id}`);
    assert.ok(source.files.length > 0, `no files for ${id}`);
    assert.ok(source.product.length > 0 && source.owner.length > 0, `missing attribution for ${id}`);
    assert.equal(source.theme, getAgentThemeVariant(id));
  }
});
