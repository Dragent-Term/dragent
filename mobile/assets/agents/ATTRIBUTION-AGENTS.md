# Agent artwork attribution (mobile live hierarchy)

Nominative use only: each mark below identifies the product named, and
remains the property of its owner. No endorsement or affiliation is
implied. If redistribution of any mark is not permitted for your
purposes, remove the corresponding path block in
`mobile/src/components/AgentMark.tsx` and the entry in
`mobile/src/components/agentArtwork.ts` + `agent-artwork.json`.

Sources are the original Dragent/marketing vendored assets. No invented vendor marks. No packages, network fetches, or runtime file reads: `AgentMark` embeds static path data only.

| Mobile id | Product | Owner | Vendored source file(s) | Theme | Mobile rendering |
|---|---|---|---|---|---|
| `dragent` | Dragent | Dragent | `dragent/brand/dragent-icon-512.png` (mobile copy `mobile/assets/agents/dragent-icon.png`) | single | Existing `DragentMark` PNG, unchanged |
| `codex` | Codex | OpenAI, L.L.C. | `dragent/app/assets/dragent/agent-images/agents/codex.svg`, `codex-white.svg` | light-dark | Existing `CodexMark` blossom path, unchanged (`#000000` / `#FFFFFF`) |
| `opencode` | OpenCode | sst | `dragent/app/assets/dragent/agent-images/agents/opencode.svg`, `opencode-white.svg` | light-dark | Existing `OpenCodeMark` rects, unchanged |
| `claude` | Claude | Anthropic, PBC | `dragent/app/assets/dragent/agent-images/agents/claude.svg` | single | Verbatim path, `viewBox 0 0 100 100`, fill `#D97757` |
| `gemini` | Gemini | Google | `dragent/app/assets/dragent/agent-images/agents/gemini.svg` | single source, theme-legible monochrome | Exact star-silhouette geometry from the source mask (`viewBox 0 0 65 65`); full four-color gradient stays in the vendored source — mobile fills `#1A73E8` (light) / `#8AB4F8` (dark) for legibility |
| `cursor` | Cursor | Cursor | `dragent/app/assets/dragent/agent-images/cursor.svg`, `cursor-white.svg` | light-dark | Verbatim path, `viewBox 0 0 167 191`, fills `#26241E` / `#D9DBE1` per catalog |
| `amp` | Amp | Amp | `dragent/app/assets/dragent/agent-images/agents/amp.svg` | single | Verbatim 4 paths, `viewBox 0 0 28 28`, brand `#F34E3F` |

Accepted aliases (live `agent_kind` normalization, see `agentArtwork.ts`):
`claude-code` → `claude`, `ampcode` → `amp`, `cursor-agent` → `cursor`,
`codex-cli` → `codex`, `gemini-cli` → `gemini`.

All other native catalog ids (39 total in `PromptAgent::ALL`, stable ids
are `command_key()`; customs are `custom:<slug>`) and any unknown string
render the honest unbranded fallback (`Icon name="agent"`), never OpenCode.

Full native catalog inspected in
`dragent/app/src/workspace/action.rs:28-68` (`PromptAgent::ALL`),
`agent_definition.rs:17-18,196-217` (stable `command_key()` ids, `custom:<slug>`),
`desktop_catalog.rs:74-87` (explicit `agent_id` only, unknown stays terminal).
The 7 ids above keep their hand-drawn marks. Every other built-in agent
(32, keyed by its command name, e.g. `crush`, `kiro-cli`, `vibe`) is drawn
from the desktop's own icon: `scripts/agent-marks-table.mjs` lists each
agent's desktop source file(s) (`dragent/app/assets/dragent/agent-images/`,
as `PromptAgent::image_path` picks them), and `scripts/sync-agent-marks.mjs`
embeds the SVGs verbatim and copies the PNGs (shrunk to at most 128 px) to
`mobile/assets/agents/marks/`, producing
`src/components/agentMarks.generated.ts`. `scripts/agent-marks.test.ts`
fails when the desktop gains an agent the phone has no icon for. Same
nominative-use terms as above. Customs and unknown strings still render the
unbranded fallback.
