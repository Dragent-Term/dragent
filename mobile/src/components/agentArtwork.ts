/**
 * Shared live agent-kind catalog for `AgentMark`.
 *
 * Pure data + normalization only (no JSX, no theme, no `preview/types`).
 * Accepts the stable native registry ids (`PromptAgent::command_key()`:
 * `dragent/claude/codex/gemini/opencode/amp/...`, customs `custom:<slug>`)
 * that the live desktop catalog publishes as `agent_kind`, plus the
 * user-facing aliases the brief requires. Unknown/custom ids normalize to
 * null so callers render an honest unbranded fallback (never OpenCode).
 *
 * Artwork geometry is verbatim path data from the vendored originals listed
 * in `mobile/assets/agents/ATTRIBUTION-AGENTS.md`:
 * - dragent: `dragent/brand/dragent-icon-512.png` (PNG, existing `DragentMark`)
 * - codex: `agent-images/agents/codex{,-white}.svg` (existing `CodexMark`)
 * - opencode: `agent-images/agents/opencode{,-white}.svg` (existing `OpenCodeMark`)
 * - claude: `agent-images/agents/claude.svg` (single variant)
 * - cursor: `agent-images/cursor{,-white}.svg` (light/dark per catalog)
 * - amp: `agent-images/agents/amp.svg` (single variant, brand `#F34E3F`)
 * - gemini: `agent-images/agents/gemini.svg` star silhouette (single catalog
 *   variant; mobile renders the exact mask geometry as a theme-legible
 *   monochrome — full gradient stays in the vendored source).
 */

export const SUPPORTED_AGENT_IDS = [
  "dragent",
  "claude",
  "codex",
  "gemini",
  "opencode",
  "amp",
  "cursor",
] as const;

export type SupportedAgentId = (typeof SUPPORTED_AGENT_IDS)[number];

/** User-facing aliases accepted in live `agent_kind` (lowercased, trimmed). */
export const AGENT_ALIASES: Readonly<Record<string, SupportedAgentId>> = Object.freeze({
  "claude-code": "claude",
  "ampcode": "amp",
  "cursor-agent": "cursor",
  "codex-cli": "codex",
  "gemini-cli": "gemini",
});

const SUPPORTED_SET: ReadonlySet<string> = new Set(SUPPORTED_AGENT_IDS);

/** Display names match the native `PromptAgent::label()` strings. */
const AGENT_LABELS: Readonly<Record<SupportedAgentId, string>> = Object.freeze({
  dragent: "Dragent",
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini CLI",
  opencode: "OpenCode",
  amp: "Amp",
  cursor: "Cursor",
});

/**
 * Normalize a live `agent_kind` (or legacy preview id) to a supported id.
 * Returns null for unknown/custom/empty input so callers fall back honestly.
 */
export function normalizeAgentKind(input: unknown): SupportedAgentId | null {
  if (typeof input !== "string") return null;
  const key = input.trim().toLowerCase();
  if (key === "") return null;
  // Custom user agents (`custom:<slug>`) have filesystem-only artwork on
  // desktop and no vendored mobile mark: never map them to a vendor.
  if (key.startsWith("custom:")) return null;
  if ((SUPPORTED_SET as ReadonlySet<string>).has(key)) return key as SupportedAgentId;
  return AGENT_ALIASES[key] ?? null;
}

export function getAgentLabel(id: SupportedAgentId): string {
  return AGENT_LABELS[id];
}

/** Theme behavior per id, mirroring `PromptAgent::image_path(is_dark)`. */
export type AgentThemeVariant = "single" | "light-dark";

const AGENT_THEME_VARIANTS: Readonly<Record<SupportedAgentId, AgentThemeVariant>> = Object.freeze({
  dragent: "single",
  claude: "single",
  codex: "light-dark",
  gemini: "single",
  opencode: "light-dark",
  amp: "single",
  cursor: "light-dark",
});

export function getAgentThemeVariant(id: SupportedAgentId): AgentThemeVariant {
  return AGENT_THEME_VARIANTS[id];
}

/** Vendored source for each mark (audit trail; no runtime reads). */
export interface AgentArtworkSource {
  readonly product: string;
  readonly owner: string;
  readonly files: readonly string[];
  readonly theme: AgentThemeVariant;
  readonly note: string;
}

export const AGENT_ARTWORK_SOURCES: Readonly<Record<SupportedAgentId, AgentArtworkSource>> = Object.freeze({
  dragent: Object.freeze({
    product: "Dragent",
    owner: "Dragent",
    files: Object.freeze(["dragent/brand/dragent-icon-512.png"]),
    theme: "single" as const,
    note: "Existing DragentMark PNG, unchanged.",
  }),
  claude: Object.freeze({
    product: "Claude",
    owner: "Anthropic, PBC",
    files: Object.freeze(["dragent/app/assets/dragent/agent-images/agents/claude.svg"]),
    theme: "single" as const,
    note: "Wikimedia Claude_AI_symbol.svg, fill #D97757, viewBox 0 0 100 100.",
  }),
  codex: Object.freeze({
    product: "Codex",
    owner: "OpenAI, L.L.C.",
    files: Object.freeze([
      "dragent/app/assets/dragent/agent-images/agents/codex.svg",
      "dragent/app/assets/dragent/agent-images/agents/codex-white.svg",
    ]),
    theme: "light-dark" as const,
    note: "OpenAI 2025 blossom, existing CodexMark path unchanged.",
  }),
  gemini: Object.freeze({
    product: "Gemini",
    owner: "Google",
    files: Object.freeze(["dragent/app/assets/dragent/agent-images/agents/gemini.svg"]),
    theme: "single" as const,
    note: "Star silhouette geometry verbatim from gemini.svg mask; monochrome theme-legible fill (full gradient in source).",
  }),
  opencode: Object.freeze({
    product: "OpenCode",
    owner: "sst",
    files: Object.freeze([
      "dragent/app/assets/dragent/agent-images/agents/opencode.svg",
      "dragent/app/assets/dragent/agent-images/agents/opencode-white.svg",
    ]),
    theme: "light-dark" as const,
    note: "sst/opencode brand page, existing OpenCodeMark rects unchanged.",
  }),
  amp: Object.freeze({
    product: "Amp",
    owner: "Amp",
    files: Object.freeze(["dragent/app/assets/dragent/agent-images/agents/amp.svg"]),
    theme: "single" as const,
    note: "ampcode.com press-kit logomark, brand #F34E3F, viewBox 0 0 28 28.",
  }),
  cursor: Object.freeze({
    product: "Cursor",
    owner: "Cursor",
    files: Object.freeze([
      "dragent/app/assets/dragent/agent-images/cursor.svg",
      "dragent/app/assets/dragent/agent-images/cursor-white.svg",
    ]),
    theme: "light-dark" as const,
    note: "Cursor icon geometry verbatim; light #26241E / dark #D9DBE1 per catalog.",
  }),
});
