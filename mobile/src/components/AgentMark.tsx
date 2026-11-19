import { Image } from "react-native";
import Svg, { ClipPath, Defs, G, Path, Rect } from "react-native-svg";
import { SvgCss } from "react-native-svg/css";
import { AGENT_MARKS, type AgentMarkArt } from "./agentMarks.generated";
import { useTheme } from "../theme";
import { Icon } from "./Icon";
import { normalizeAgentKind } from "./agentArtwork";

/**
 * Exact product artwork from the repo catalogs, used nominatively to identify
 * which agent or tool owns a tab (see mobile/assets/agents/ATTRIBUTION-AGENTS.md
 * and dragent/app/assets/dragent/icons/ATTRIBUTION-LOGOS.md):
 * - Dragent → dragent/brand/dragent-icon-512.png
 * - Codex → agent-images/agents/codex{,-white}.svg (OpenAI 2025 blossom)
 * - OpenCode → agent-images/agents/opencode{,-white}.svg (sst brand page)
 * - Claude → agent-images/agents/claude.svg (Anthropic)
 * - Gemini → agent-images/agents/gemini.svg star geometry (monochrome)
 * - Cursor → agent-images/cursor{,-white}.svg
 * - Amp → agent-images/agents/amp.svg (brand #F34E3F)
 * - Terminal tab → agent-images/terminal.png (Terminal app icon)
 * - Browser tab → the catalog globe (lucide, as in icons/globe.svg)
 * Unknown/custom kinds render an honest unbranded fallback (never OpenCode).
 */

// OpenAI 2025 blossom symbol, verbatim path from codex.svg (viewBox 1.68 1.75 16.65 16.5).
const CODEX_PATH =
  "M11.248 18.25q-.825 0-1.568-.314a4.3 4.3 0 0 1-1.32-.874 4 4 0 0 1-1.304.214 4 4 0 0 1-2.046-.544 4.27 4.27 0 0 1-1.518-1.485 4 4 0 0 1-.56-2.095q0-.48.131-1.04A4.4 4.4 0 0 1 2.04 10.71a4.07 4.07 0 0 1 .017-3.4 4.2 4.2 0 0 1 1.056-1.418 3.8 3.8 0 0 1 1.6-.842 3.9 3.9 0 0 1 .76-1.683q.593-.759 1.451-1.188a4.04 4.04 0 0 1 1.832-.429q.825 0 1.567.313.742.314 1.32.875a4 4 0 0 1 1.304-.215q1.106 0 2.046.545a4.14 4.14 0 0 1 1.501 1.485q.578.941.578 2.095 0 .48-.132 1.04.66.61 1.023 1.419.363.792.363 1.666 0 .892-.38 1.717a4.3 4.3 0 0 1-1.072 1.435 3.8 3.8 0 0 1-1.584.825 3.8 3.8 0 0 1-.775 1.683 4.06 4.06 0 0 1-1.436 1.188 4.04 4.04 0 0 1-1.832.429m-4.076-2.062q.825 0 1.435-.347l3.103-1.782a.36.36 0 0 0 .164-.313v-1.42L7.881 14.62a.67.67 0 0 1-.726 0l-3.118-1.798a.5.5 0 0 1-.017.115v.198q0 .841.396 1.551.413.693 1.139 1.089a3.2 3.2 0 0 0 1.617.412m.165-2.69a.4.4 0 0 0 .181.05q.083 0 .165-.05l1.238-.71-3.977-2.31a.7.7 0 0 1-.363-.643v-3.58q-.825.362-1.32 1.122a2.9 2.9 0 0 0-.495 1.65q0 .809.413 1.55.412.743 1.072 1.123zm3.91 3.663q.875 0 1.585-.396a2.96 2.96 0 0 0 1.534-2.64v-3.564a.32.32 0 0 0-.165-.297l-1.254-.726v4.604a.7.7 0 0 1-.363.643l-3.119 1.799a3 3 0 0 0 1.783.577m.627-6.039V8.878L10.01 7.822 8.129 8.878v2.244l1.881 1.056zM7.057 5.859a.7.7 0 0 1 .363-.644l3.119-1.798a3 3 0 0 0-1.782-.578q-.874 0-1.584.396A2.96 2.96 0 0 0 6.05 4.324a3.07 3.07 0 0 0-.396 1.551v3.547q0 .199.165.314l1.237.726zm8.383 7.887q.825-.364 1.303-1.123.495-.758.495-1.65a3.15 3.15 0 0 0-.412-1.55q-.413-.743-1.073-1.123l-3.086-1.782q-.099-.065-.181-.049a.3.3 0 0 0-.165.05l-1.238.692 3.993 2.327a.6.6 0 0 1 .264.264.64.64 0 0 1 .1.363zm-3.317-8.382a.63.63 0 0 1 .726 0l3.135 1.831v-.297q0-.792-.396-1.501a2.86 2.86 0 0 0-1.105-1.155q-.71-.43-1.65-.43-.825 0-1.436.347L8.294 5.941a.36.36 0 0 0-.165.314v1.418z";

function CodexMark({ size, dark }: { size: number; dark: boolean }) {
  return (
    <Svg width={size} height={size} viewBox="1.68 1.75 16.65 16.5">
      <Path d={CODEX_PATH} fill={dark ? "#FFFFFF" : "#000000"} />
    </Svg>
  );
}

function OpenCodeMark({ size, dark }: { size: number; dark: boolean }) {
  const inner = dark ? "#4B4646" : "#BCBBBB";
  const outer = dark ? "#F1ECEC" : "#211E1E";
  return (
    <Svg width={(size * 32) / 40} height={size} viewBox="0 0 32 40" fill="none">
      <Defs>
        <ClipPath id="opencode-mark-clip">
          <Rect width="32" height="40" fill="white" />
        </ClipPath>
      </Defs>
      <G clipPath="url(#opencode-mark-clip)">
        <Path d="M24 32H8V16H24V32Z" fill={inner} />
        <Path d="M24 8H8V32H24V8ZM32 40H0V0H32V40Z" fill={outer} />
      </G>
    </Svg>
  );
}

// Claude mark: verbatim geometry from agent-images/agents/claude.svg
// (viewBox 0 0 100 100, single catalog variant, fill #D97757).
const CLAUDE_PATH =
  "m19.6 66.5 19.7-11 .3-1-.3-.5h-1l-3.3-.2-11.2-.3L14 53l-9.5-.5-2.4-.5L0 49l.2-1.5 2-1.3 2.9.2 6.3.5 9.5.6 6.9.4L38 49.1h1.6l.2-.7-.5-.4-.4-.4L29 41l-10.6-7-5.6-4.1-3-2-1.5-2-.6-4.2 2.7-3 3.7.3.9.2 3.7 2.9 8 6.1L37 36l1.5 1.2.6-.4.1-.3-.7-1.1L33 25l-6-10.4-2.7-4.3-.7-2.6c-.3-1-.4-2-.4-3l3-4.2L28 0l4.2.6L33.8 2l2.6 6 4.1 9.3L47 29.9l2 3.8 1 3.4.3 1h.7v-.5l.5-7.2 1-8.7 1-11.2.3-3.2 1.6-3.8 3-2L61 2.6l2 2.9-.3 1.8-1.1 7.7L59 27.1l-1.5 8.2h.9l1-1.1 4.1-5.4 6.9-8.6 3-3.5L77 13l2.3-1.8h4.3l3.1 4.7-1.4 4.9-4.4 5.6-3.7 4.7-5.3 7.1-3.2 5.7.3.4h.7l12-2.6 6.4-1.1 7.6-1.3 3.5 1.6.4 1.6-1.4 3.4-8.2 2-9.6 2-14.3 3.3-.2.1.2.3 6.4.6 2.8.2h6.8l12.6 1 3.3 2 1.9 2.7-.3 2-5.1 2.6-6.8-1.6-16-3.8-5.4-1.3h-.8v.4l4.6 4.5 8.3 7.5L89 80.1l.5 2.4-1.3 2-1.4-.2-9.2-7-3.6-3-8-6.8h-.5v.7l1.8 2.7 9.8 14.7.5 4.5-.7 1.4-2.6 1-2.7-.6-5.8-8-6-9-4.7-8.2-.5.4-2.9 30.2-1.3 1.5-3 1.2-2.5-2-1.4-3 1.4-6.2 1.6-8 1.3-6.4 1.2-7.9.7-2.6v-.2H49L43 72l-9 12.3-7.2 7.6-1.7.7-3-1.5.3-2.8L24 86l10-12.8 6-7.9 4-4.6-.1-.5h-.3L17.2 77.4l-4.7.6-2-2 .2-3 1-1 8-5.5Z";

function ClaudeMark({ size }: { size: number }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 100 100">
      <Path d={CLAUDE_PATH} fill="#D97757" />
    </Svg>
  );
}

// Gemini mark: exact star silhouette geometry from the mask in
// agent-images/agents/gemini.svg (viewBox 0 0 65 65). The vendored source is
// a four-color gradient; mobile renders the verbatim geometry as a
// theme-legible monochrome so the shape stays exact on both themes.
const GEMINI_STAR_PATH =
  "M32.447 0c.68 0 1.273.465 1.439 1.125a38.904 38.904 0 001.999 5.905c2.152 5 5.105 9.376 8.854 13.125 3.751 3.75 8.126 6.703 13.125 8.855a38.98 38.98 0 005.906 1.999c.66.166 1.124.758 1.124 1.438 0 .68-.464 1.273-1.125 1.439a38.902 38.902 0 00-5.905 1.999c-5 2.152-9.375 5.105-13.125 8.854-3.749 3.751-6.702 8.126-8.854 13.125a38.973 38.973 0 00-2 5.906 1.485 1.485 0 01-1.438 1.124c-.68 0-1.272-.464-1.438-1.125a38.913 38.913 0 00-2-5.905c-2.151-5-5.103-9.375-8.854-13.125-3.75-3.749-8.125-6.702-13.125-8.854a38.973 38.973 0 00-5.905-2A1.485 1.485 0 010 32.448c0-.68.465-1.272 1.125-1.438a38.903 38.903 0 005.905-2c5-2.151 9.376-5.104 13.125-8.854 3.75-3.749 6.703-8.125 8.855-13.125a38.972 38.972 0 001.999-5.905A1.485 1.485 0 0132.447 0z";

function GeminiMark({ size, dark }: { size: number; dark: boolean }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 65 65">
      <Path d={GEMINI_STAR_PATH} fill={dark ? "#8AB4F8" : "#1A73E8"} />
    </Svg>
  );
}

// Cursor mark: verbatim geometry from agent-images/cursor.svg
// (viewBox 0 0 167 191). Light/dark fills match cursor{,-white}.svg.
const CURSOR_PATH =
  "M80.88 0H85.8933C110.227 14.16 134.653 28.16 159 42.2933C161.653 44.0133 164.72 45.5733 166.293 48.4533C167.12 52.2267 166.747 56.1333 166.813 59.9733C166.76 84.84 166.773 109.707 166.813 134.573C166.76 137.12 166.853 139.72 166.307 142.24C164.707 144.72 162 146.16 159.573 147.693C138.707 159.627 117.933 171.733 97.1333 183.773C92.7067 186.24 88.48 189.213 83.6533 190.893C80.68 190.467 78.0933 188.76 75.4933 187.387C54.4133 174.96 33.0933 162.96 12 150.56C7.8 148.213 3.72 145.64 0 142.573V48.1333C5.10667 43.0933 11.7467 40.16 17.7867 36.44C38.8 24.2533 59.76 11.9733 80.88 0ZM7.49333 50.72C11.04 54.1867 15.84 55.7733 19.9467 58.4C40.8 70.7467 62 82.52 82.7333 95.0667C83.8133 101.093 83.3333 107.24 83.4 113.333C83.4267 134.187 83.3467 155.053 83.3867 175.907C83.3867 178.253 83.6267 180.587 84.0133 182.893C85.5733 180.8 87.0533 178.64 88.32 176.36C110.76 137.44 133.24 98.56 155.533 59.56C157.187 56.8267 158.453 53.8933 159.427 50.8533C151.213 50.2 142.973 50.5467 134.747 50.4933C98.28 50.4667 61.8 50.4667 25.32 50.4933C19.3867 50.6933 13.4267 50.04 7.49333 50.72Z";

function CursorMark({ size, dark }: { size: number; dark: boolean }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 167 191">
      <Path d={CURSOR_PATH} fill={dark ? "#D9DBE1" : "#26241E"} />
    </Svg>
  );
}

// Amp mark: verbatim geometry from agent-images/agents/amp.svg
// (viewBox 0 0 28 28, single catalog variant, brand #F34E3F).
const AMP_PATHS: readonly string[] = [
  "M13.9197 13.61L17.3816 26.566L14.242 27.4049L11.2645 16.2643L0.119926 13.2906L0.957817 10.15L13.9197 13.61Z",
  "M13.7391 16.0892L4.88169 24.9056L2.58872 22.6019L11.4461 13.7865L13.7391 16.0892Z",
  "M18.9386 8.58315L22.4005 21.5392L19.2609 22.3781L16.2833 11.2374L5.13879 8.26381L5.97668 5.12318L18.9386 8.58315Z",
  "M23.9803 3.55632L27.4422 16.5124L24.3025 17.3512L21.325 6.21062L10.1805 3.23698L11.0183 0.0963593L23.9803 3.55632Z",
];

function AmpMark({ size }: { size: number }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 28 28" fill="none">
      {AMP_PATHS.map((d) => (
        <Path key={d} d={d} fill="#F34E3F" />
      ))}
    </Svg>
  );
}

/** Every other agent the desktop knows, drawn from the desktop's own icon. */
function DesktopAgentMark({ art, size }: { art: AgentMarkArt; size: number }) {
  if (art.kind === "svg") return <SvgCss xml={art.svg} width={size} height={size} />;
  return (
    <Image
      source={art.png}
      accessibilityIgnoresInvertColors
      resizeMode="contain"
      style={{ width: size, height: size }}
    />
  );
}

/** The desktop icon for an agent's command name (`crush`, `kiro-cli`, …), if any. */
export function desktopAgentMark(agent: string, dark: boolean): AgentMarkArt | null {
  const entry = AGENT_MARKS[agent.trim().toLowerCase()];
  if (!entry) return null;
  return dark && entry.dark ? entry.dark : entry.light;
}

/** Honest unbranded fallback for unknown/custom kinds (never a vendor mark). */
function UnknownAgentMark({ size = 20 }: { size?: number }) {
  return <Icon name="agent" size={size} tone="textMuted" />;
}

/** Exact Dragent product icon (dragent/brand/dragent-icon-512.png). */
export function DragentMark({ size = 20 }: { size?: number }) {
  return (
    <Image
      source={require("../../assets/agents/dragent-icon.png")}
      accessibilityIgnoresInvertColors
      style={{ width: size, height: size, borderRadius: size * 0.22 }}
    />
  );
}

/** Exact Terminal app icon from the catalog (agent-images/terminal.png). */
export function TerminalMark({ size = 20 }: { size?: number }) {
  return (
    <Image
      source={require("../../assets/agents/terminal-app-icon.png")}
      accessibilityIgnoresInvertColors
      style={{ width: size, height: size, borderRadius: size * 0.22 }}
    />
  );
}

/** The catalog browser glyph (icons/globe.svg). */
export function BrowserMark({ size = 20 }: { size?: number }) {
  return <Icon name="browser" size={size} tone="textMuted" />;
}

/** Per-agent product mark: exact vendor artwork, light/dark variants from the catalog. */
export function AgentMark({ agent, size = 20 }: { agent: string; size?: number }) {
  const { theme } = useTheme();
  const dark = theme.mode !== "light";
  const kind = normalizeAgentKind(agent);
  if (kind === "dragent") return <DragentMark size={size} />;
  if (kind === "codex") return <CodexMark size={size} dark={dark} />;
  if (kind === "opencode") return <OpenCodeMark size={size} dark={dark} />;
  if (kind === "claude") return <ClaudeMark size={size} />;
  if (kind === "gemini") return <GeminiMark size={size} dark={dark} />;
  if (kind === "cursor") return <CursorMark size={size} dark={dark} />;
  if (kind === "amp") return <AmpMark size={size} />;
  const art = desktopAgentMark(agent, dark);
  if (art) return <DesktopAgentMark art={art} size={size} />;
  return <UnknownAgentMark size={size} />;
}
