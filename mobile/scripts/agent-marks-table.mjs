// Every desktop agent the phone draws from the desktop's own icon (the seven
// hand-drawn marks in src/components/AgentMark.tsx aside), by the command
// name the desktop's catalog sends. Mirrors `PromptAgent::image_path` in
// dragent/app/src/workspace/action.rs; scripts/agent-marks.test.ts keeps the
// two in step.
// [command name, label, light image, dark image or null]
export const AGENTS = [
  ["copilot", "GitHub Copilot", "agents/copilot.svg", "agents/copilot-white.svg"],
  ["agy", "Antigravity", "agents/antigravity.svg", null],
  ["auggie", "Auggie", "Auggie.svg", "Auggie-white.svg"],
  ["autohand", "Autohand", "autohand.svg", "autohand-white.svg"],
  ["crush", "Crush", "charm.png", null],
  ["cline", "Cline", "cline.png", null],
  ["codebuddy", "CodeBuddy", "codebuddy.svg", null],
  ["codebuff", "Codebuff", "codebuff.png", null],
  ["command-code", "Command Code", "commandcode.svg", null],
  ["cn", "Continue", "continue.png", null],
  ["devin", "Devin", "agents/devin.svg", "agents/devin-white.svg"],
  ["droid", "Droid", "agents/droid.svg", "agents/droid-white.svg"],
  ["freebuff", "Freebuff", "codebuff.png", null],
  ["goose", "Goose", "goose.png", null],
  ["grok", "Grok", "agents/grok.svg", "agents/grok-white.svg"],
  ["hermes", "Hermes", "agents/hermes.png", null],
  ["jules", "Jules", "jules.svg", null],
  ["junie", "Junie", "junie-color.png", null],
  ["kilo", "Kilocode", "kilocode.png", null],
  ["kimi", "Kimi", "agents/kimi.svg", "agents/kimi-white.svg"],
  ["kiro-cli", "Kiro", "agents/kiro.svg", null],
  ["letta", "Letta", "letta.svg", "letta-white.svg"],
  ["mimo", "MiMo Code", "mimocode.svg", "mimocode-white.svg"],
  ["vibe", "Mistral Vibe", "agents/vibe.svg", null],
  ["muse", "Muse Code", "agents/muse.png", null],
  ["omp", "Oh My Pi", "agents/pi.svg", "agents/pi-white.svg"],
  ["pi", "Pi", "agents/pi.svg", "agents/pi-white.svg"],
  ["prime-agent", "Prime Agent", "prime-agent.svg", "prime-agent-white.svg"],
  ["qodercli", "Qoder CLI", "qoder.svg", null],
  ["qwen", "Qwen Code", "qwen.svg", null],
  ["rovodev", "Rovo Dev", "atlassian.png", null],
  ["zero", "Zero", "zero.svg", null],
];
