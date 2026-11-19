"use client";

import { useEffect, useReducer, useState } from "react";
import Image from "next/image";
import { agentMarkClass } from "@/lib/agents";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import {
  ArrowDownToLine,
  ArrowDownWideNarrow,
  ArrowLeft,
  Braces,
  ChevronDown,
  CircleHelp,
  Clock,
  FilePlus,
  FileText,
  FolderPlus,
  GitCompareArrows,
  GitPullRequest,
  Layers,
  ListTodo,
  LogOut,
  PanelLeft,
  RefreshCw,
  Server,
  Search,
  Plus,
  Sparkles,
  Copy,
  MoreVertical,
  Minus,
  Zap,
  Check,
  Play,
  Pause,
  GitBranch,
  FilePen,
  FileSearch,
  SquareTerminal,
  ChevronRight,
  ChevronUp,
  Folder,
  Mic,
  ArrowUp,
} from "lucide-react";

/* ── The self-playing Dragent app demo ────────────────────────────────────
   A faithful recreation of the real app shell that plays TWO scenes on a loop,
   switching via the input's mode pill (shell ⇄ agent), exactly like the app:

   • Shell — types `yarn dev`, streams logs, hits a port error, the agent
     diagnoses it and one-click-fixes it.
   • Agent — you ask in plain English; the agent plans, explores, EDITS a file
     (with a real +/- diff), runs the test command, and reports back.

   All live DOM, so it stays crisp at any size. ──────────────────────────── */

type Mode = "shell" | "agent";

const CMD = "yarn dev";
const PROMPT = "add a test for the cache cleanup";
const STREAM = [
  "yarn run v1.22.22",
  "$ next dev",
  "▲ Next.js 16.2.9  (Turbopack)",
  "- Local:      http://localhost:3000",
];

// The diff the agent writes in queue.test.ts.
const DIFF: { t: "-" | "+" | " "; s: string }[] = [
  { t: "-", s: "const deleteMock = caches.delete as Mock;" },
  { t: "+", s: "const deleteMock = vi.fn();" },
  { t: "+", s: "(caches.delete as Mock).mockImplementation(deleteMock);" },
  { t: " ", s: "" },
  { t: " ", s: 'await cleanupAppCaches(["orbit-web-v2"]);' },
  { t: "+", s: "expect(deleteMock).toHaveBeenCalledTimes(1);" },
];

type ShellKey =
  | "idle"
  | "typing"
  | "submit"
  | "streaming"
  | "error"
  | "diagnose"
  | "fixing"
  | "resolved";
type AgentKey =
  | "idle"
  | "typing"
  | "submit"
  | "thinking"
  | "editing"
  | "running"
  | "message"
  | "done";

const SHELL: { key: ShellKey; ms: number }[] = [
  { key: "idle", ms: 1200 },
  { key: "typing", ms: 1000 },
  { key: "submit", ms: 300 },
  { key: "streaming", ms: 1800 },
  { key: "error", ms: 1100 },
  { key: "diagnose", ms: 1400 },
  { key: "fixing", ms: 1200 },
  { key: "resolved", ms: 2400 },
];
const AGENT: { key: AgentKey; ms: number }[] = [
  { key: "idle", ms: 1200 },
  { key: "typing", ms: 1700 },
  { key: "submit", ms: 400 },
  { key: "thinking", ms: 1400 },
  { key: "editing", ms: 2600 },
  { key: "running", ms: 1700 },
  { key: "message", ms: 1700 },
  { key: "done", ms: 2600 },
];

const stepsFor = (m: Mode) => (m === "shell" ? SHELL : AGENT);

// `seq` changes on every step, so per-step progress can tell when it is stale.
type State = { mode: Mode; i: number; seq: number };
type Action = { type: "next" } | { type: "set"; mode: Mode };

function reducer(s: State, a: Action): State {
  const seq = s.seq + 1;
  if (a.type === "set") return { mode: a.mode, i: 0, seq };
  const steps = stepsFor(s.mode);
  if (s.i + 1 >= steps.length)
    return { mode: s.mode === "shell" ? "agent" : "shell", i: 0, seq };
  return { ...s, i: s.i + 1, seq };
}

export function AppDemo() {
  const [{ mode, i, seq }, dispatch] = useReducer(reducer, { mode: "shell", i: 0, seq: 0 });
  const [playing, setPlaying] = useState(true);
  // How far the current step's reveal (typing, log stream, diff) has got.
  // Tagged with its step, so each new step starts from zero.
  const [progress, setProgress] = useState({ seq: 0, n: 0 });
  const reduced = !!useReducedMotion();

  const steps = stepsFor(mode);
  const step = steps[i];
  const key = step.key;
  const idxOf = (k: string) => steps.findIndex((s) => s.key === k);
  const past = (k: string) => reduced || i > idxOf(k);
  const at = (k: string) => (reduced ? false : key === k);

  // Main timeline.
  useEffect(() => {
    if (reduced || !playing) return;
    const t = setTimeout(() => dispatch({ type: "next" }), step.ms);
    return () => clearTimeout(t);
  }, [mode, i, playing, reduced, step.ms]);

  // Reveal the current step's items one at a time: the command or prompt
  // (typing), log lines (shell streaming), diff lines (agent editing).
  const typeTarget = mode === "shell" ? CMD : PROMPT;
  const items =
    key === "typing"
      ? typeTarget.length
      : key === "streaming"
        ? STREAM.length
        : key === "editing"
          ? DIFF.length
          : 0;
  useEffect(() => {
    if (reduced || !playing || items === 0) return;
    let n = 0;
    const id = setInterval(() => {
      n += 1;
      setProgress({ seq, n });
      if (n >= items) clearInterval(id);
    }, step.ms / (items + 1));
    return () => clearInterval(id);
  }, [seq, playing, reduced, items, step.ms]);

  const shown = progress.seq === seq ? progress.n : 0;
  // The typed text stays in the input until the scene starts over.
  const typed = reduced
    ? ""
    : at("typing")
      ? typeTarget.slice(0, shown)
      : past("typing")
        ? typeTarget
        : "";
  const logs = key === "streaming" ? shown : 0;
  const diff = key === "editing" ? shown : 0;

  const switchMode = (m: Mode) => {
    if (m !== mode) dispatch({ type: "set", mode: m });
  };

  const hasError =
    mode === "shell" &&
    (past("error") || at("error") || past("diagnose") || at("diagnose")) &&
    !(at("resolved") || past("resolved"));

  const cursor = cursorPos(mode, key);
  const typingNow = at("typing") || at("idle") || at("submit");
  // The selected workspace's status, as its sidebar card and tab show it:
  // working while a scene runs, needs input while the fix waits on a click,
  // failed on the error, ready for review when done.
  const status: WsStatus = reduced
    ? "review"
    : key === "error"
      ? "failed"
      : key === "diagnose"
        ? "permission"
        : key === "resolved" || key === "done"
          ? "review"
          : key === "idle" || key === "typing"
            ? "idle"
            : "working";

  return (
    <div className="product-demo relative">
      <div className="relative overflow-hidden rounded-2xl border border-white/10 bg-[#0c1015] shadow-[0_40px_120px_-30px_rgba(0,0,0,0.9)]">
        {/* body — the window controls live in the left sidebar, as in the app */}
        <div className="flex h-[600px] text-[12.5px] sm:h-[680px]">
          <LeftSidebar status={status} />

          <div className="relative flex min-w-0 flex-1 flex-col bg-[#0c1015]">
            {/* workspace tab strip (workspace_tab_strip.rs) */}
            <div className="flex h-9 shrink-0 items-stretch border-b border-white/8">
              <span className="flex items-center pl-3 pr-2 sm:hidden">
                <TrafficLights />
              </span>
              <TabChip
                active
                status={status}
                icon={mode === "agent" ? "/dragent-mark.png" : undefined}
                label={mode === "agent" ? "Dragent" : "yarn dev"}
              />
              <TabChip icon="/agents/claude.svg" label="Claude" status="idle" />
              <span className="flex items-center px-2.5 text-white/40">
                <Plus size={13} />
              </span>
              <span className="ml-auto flex items-center pr-3">
                <MoreVertical size={13} className="text-white/30" />
              </span>
            </div>

            {/* scene */}
            <div className="flex-1 overflow-hidden px-4 py-3 font-[family-name:var(--font-mono-jb)] leading-5">
              {mode === "shell" ? (
                <ShellScene
                  visibleLogs={reduced ? STREAM.length : past("streaming") ? STREAM.length : key === "streaming" ? logs : 0}
                  showBlockB={reduced || past("submit") || at("submit")}
                  showError={reduced || past("error") || at("error")}
                  showFix={reduced || past("diagnose") || at("diagnose")}
                  resolved={reduced || at("resolved") || past("resolved")}
                  fixing={at("fixing")}
                />
              ) : (
                <AgentScene
                  prompt={PROMPT}
                  showTask={reduced || past("submit") || at("submit")}
                  showThinking={reduced || past("thinking") || at("thinking")}
                  thinking={at("thinking")}
                  showEdit={reduced || past("editing") || at("editing")}
                  diffCount={reduced ? DIFF.length : past("editing") ? DIFF.length : key === "editing" ? diff : 0}
                  showRun={reduced || past("running") || at("running")}
                  showMessage={reduced || past("message") || at("message")}
                  done={reduced || at("done") || past("done")}
                />
              )}
            </div>

            <InputBar mode={mode} typed={typed} typingNow={typingNow} onSwitch={switchMode} />

            {/* animated cursor — positioned within this column, so the
                sidebars showing or hiding never move its targets */}
            {!reduced && (
              <motion.div
                className="pointer-events-none absolute z-30"
                initial={false}
                animate={{ left: `${cursor.x}%`, top: `${cursor.y}%` }}
                transition={{ type: "spring", stiffness: 90, damping: 18, mass: 0.7 }}
              >
                <Cursor clicking={at("fixing")} />
              </motion.div>
            )}
          </div>

          <AgentRail />
          <RightSidebar hasError={hasError} />
        </div>
      </div>

      {!reduced && (
        <button
          onClick={() => setPlaying((p) => !p)}
          aria-label={playing ? "Pause demo" : "Play demo"}
          className="absolute -bottom-4 right-4 z-40 inline-flex h-9 w-9 items-center justify-center rounded-full border border-white/12 bg-[#0b0b0e] text-white/70 shadow-lg transition-colors hover:text-white"
        >
          {playing ? <Pause size={15} /> : <Play size={15} className="ml-0.5" />}
        </button>
      )}
    </div>
  );
}

/* ── Workspace tab chip ─────────────────────────────────────────────────── */

// The live agent status tints the chip's top rail and icon, and its trailing
// dot glows while the agent works — so a tab reads without hovering.
function TabChip({
  label,
  icon,
  status,
  active,
}: {
  label: string;
  icon?: string;
  status: WsStatus;
  active?: boolean;
}) {
  const reduced = !!useReducedMotion();
  const color = statusColor(status);
  const working = status === "working";
  const rail = color ?? (active ? ACCENT : "transparent");
  return (
    <div
      className="flex w-[150px] shrink-0 items-center gap-[7px] border-r border-white/[0.07] pl-[11px] pr-1.5 xl:w-[205px]"
      style={{
        background: active ? "#0c1015" : "transparent",
        boxShadow: `inset 0 2px 0 ${rail}`,
      }}
    >
      {icon ? (
        <Image src={icon} alt="" width={14} height={14} className="shrink-0" />
      ) : (
        <SquareTerminal size={13} className="shrink-0" style={{ color: color ?? "rgba(255,255,255,0.6)" }} />
      )}
      <span
        className={`min-w-0 flex-1 truncate text-[12px] ${active ? "text-white/90" : "text-white/55"}`}
      >
        {label}
      </span>
      {color && (
        <span className="flex shrink-0 items-center pl-5 pr-2">
          {working ? (
            <motion.span
              className="flex rounded-full p-[2.5px]"
              style={{ background: `${color}28` }}
              animate={reduced ? undefined : { opacity: [0.6, 1, 0.6] }}
              transition={{ duration: 1.4, repeat: Infinity, ease: "easeInOut" }}
            >
              <span className="h-[5px] w-[5px] rounded-full" style={{ background: color }} />
            </motion.span>
          ) : (
            <span className="h-1.5 w-1.5 rounded-full" style={{ background: color }} />
          )}
        </span>
      )}
    </div>
  );
}

/* ── Input bar (distinct shell vs agent layouts) ─────────────────────────── */

function InputBar({
  mode,
  typed,
  typingNow,
  onSwitch,
}: {
  mode: Mode;
  typed: string;
  typingNow: boolean;
  onSwitch: (m: Mode) => void;
}) {
  const agent = mode === "agent";
  return (
    <div className="border-t border-white/8 px-4 py-2.5">
      {/* metadata pills — shared by both modes */}
      <div className="flex flex-wrap items-center gap-1.5 font-[family-name:var(--font-mono-jb)] text-[10.5px]">
        <MetaPill>
          <Folder size={10} className="text-white/40" />
          <span className="text-white/55">~/dev/orbit-web</span>
        </MetaPill>
        <MetaPill>
          <GitBranch size={10} className="text-white/40" />
          <span className="text-white/55">feat/cache-cleanup</span>
        </MetaPill>
        <MetaPill>
          <span className="h-1.5 w-1.5 rounded-full bg-[#febc2e]" />
          <span className="text-white/55">3</span>
        </MetaPill>
        <MetaPill>
          <span className="text-[#34d399]">+155</span>
          <span className="text-[#f4677a]">-0</span>
        </MetaPill>
      </div>

      {/* prompt row */}
      <div className="mt-2.5 flex items-center gap-2">
        <ChevronRight size={13} className="shrink-0 text-[#4ade80]" />
        <span className="min-w-0 flex-1 truncate font-[family-name:var(--font-mono-jb)] text-[12.5px]">
          {agent ? (
            typed ? (
              <span className="text-white/85">{typed}</span>
            ) : (
              <span className="text-white/30">Ask Dragent…</span>
            )
          ) : (
            <ShellText typed={typed} />
          )}
          {typingNow && (
            <span className="ml-0.5 inline-block h-[1.05em] w-[7px] animate-blink bg-white/70 align-middle" />
          )}
        </span>
        <button
          onClick={() => onSwitch(agent ? "shell" : "agent")}
          title="Switch mode"
          className={`shrink-0 rounded-md px-2 py-0.5 font-[family-name:var(--font-mono-jb)] text-[10.5px] transition-colors ${
            agent
              ? "bg-[#4fd4f6]/12 text-[#9be7ff] hover:bg-[#4fd4f6]/20"
              : "bg-white/[0.06] text-white/50 hover:text-white/80"
          }`}
        >
          {mode}
        </button>
      </div>

      {/* turn / token meta */}
      <div className="mt-1.5 font-[family-name:var(--font-mono-jb)] text-[10px] text-white/30">
        Turn 11/200 · ~1.2M tokens
      </div>

      {/* agent-only toolbar */}
      {agent && (
        <div className="mt-2.5 flex items-center gap-2">
          <button className="flex h-6 w-6 items-center justify-center rounded-md border border-white/10 text-white/45">
            <Plus size={13} />
          </button>
          <button
            onClick={() => onSwitch("shell")}
            className="flex items-center gap-1 rounded-md border border-white/10 bg-white/[0.03] px-2 py-1 text-[11px] text-white/70"
          >
            Agent <ChevronUp size={11} className="text-white/40" />
          </button>
          <button className="hidden items-center gap-1 rounded-md border border-white/10 bg-white/[0.03] px-2 py-1 text-[11px] text-white/70 sm:flex">
            Claude Fable 5 <ChevronUp size={11} className="text-white/40" />
          </button>
          <span className="hidden font-[family-name:var(--font-mono-jb)] text-[10.5px] text-white/30 md:inline">
            37% · 46.9K/128.0K
          </span>
          <span className="ml-auto flex items-center gap-2">
            <Mic size={14} className="text-white/40" />
            <span className="flex h-7 w-7 items-center justify-center rounded-md bg-white/10 text-white/80">
              <ArrowUp size={14} />
            </span>
          </span>
        </div>
      )}
    </div>
  );
}

function MetaPill({ children }: { children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-md border border-white/8 bg-white/[0.03] px-2 py-1">
      {children}
    </span>
  );
}

function ShellText({ typed }: { typed: string }) {
  if (!typed) return <span className="text-white/30">run a command</span>;
  const [first, ...rest] = typed.split(" ");
  return (
    <>
      <span className="text-[#4ade80]">{first}</span>
      {rest.length > 0 && <span className="text-white/85"> {rest.join(" ")}</span>}
    </>
  );
}

/* ── Shell scene ─────────────────────────────────────────────────────────── */

function ShellScene({
  visibleLogs,
  showBlockB,
  showError,
  showFix,
  resolved,
  fixing,
}: {
  visibleLogs: number;
  showBlockB: boolean;
  showError: boolean;
  showFix: boolean;
  resolved: boolean;
  fixing: boolean;
}) {
  return (
    <>
      <Block accent="ok" header="npm i" meta={<Meta time="2.8s" ok />}>
        <div className="text-white/55">up to date, audited 439 packages in 2s</div>
        <div className="text-white/55">found 0 vulnerabilities</div>
      </Block>

      <AnimatePresence>
        {showBlockB && (
          <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.3 }}>
            <Block
              accent={resolved ? "ok" : showError ? "err" : "run"}
              header="yarn dev"
              meta={
                resolved ? (
                  <Meta time="1.4s" ok />
                ) : (
                  <span className="flex items-center gap-2 text-white/30">
                    <Copy size={12} />
                    <span className="text-[11px]">running</span>
                    <Minus size={12} />
                  </span>
                )
              }
            >
              {STREAM.slice(0, visibleLogs).map((l, i) => (
                <div key={i} className="text-white/55">
                  {l.includes("Next.js") ? (
                    <>
                      <span className="text-[#c4b5fd]">▲ Next.js 16.2.9</span>
                      <span className="text-white/40">  (Turbopack)</span>
                    </>
                  ) : (
                    l
                  )}
                </div>
              ))}
              {showError && !resolved && (
                <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} className="text-[#f4677a]">
                  Error: listen EADDRINUSE: address already in use :::3000
                </motion.div>
              )}
              {resolved && (
                <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} className="text-[#34d399]">
                  ✓ Ready in 590ms — http://localhost:3000
                </motion.div>
              )}
            </Block>
          </motion.div>
        )}
      </AnimatePresence>

      <AnimatePresence>
        {showFix && !resolved && (
          <motion.div
            initial={{ opacity: 0, height: 0, y: -6 }}
            animate={{ opacity: 1, height: "auto", y: 0 }}
            exit={{ opacity: 0, height: 0 }}
            transition={{ duration: 0.4, ease: [0.22, 1, 0.36, 1] }}
            className="overflow-hidden"
          >
            <div className="silver-border mt-1 rounded-xl p-3">
              <div className="flex items-center gap-2">
                <Sparkles size={14} className="text-white/85" />
                <span className="font-[family-name:var(--font-outfit)] text-[12.5px] font-medium text-white">
                  Error diagnosed
                </span>
                <span className="ml-auto font-[family-name:var(--font-outfit)] text-[10.5px] text-white/40">
                  Dragent Agent
                </span>
              </div>
              <p className="mt-1.5 font-[family-name:var(--font-inter)] text-[12px] leading-relaxed text-white/60">
                Port 3000 is held by <span className="text-white/85">node</span> (pid 48213). I can free it and restart.
              </p>
              <motion.button
                animate={fixing ? { scale: [1, 0.94, 1] } : { scale: 1 }}
                transition={{ duration: 0.35 }}
                className="mt-2.5 inline-flex items-center gap-1.5 rounded-lg bg-white px-3 py-1.5 font-[family-name:var(--font-outfit)] text-[11.5px] font-semibold text-[#05070a]"
              >
                <Zap size={12} /> Kill port and retry
              </motion.button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {resolved && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          className="mt-1 inline-flex items-center gap-1.5 rounded-lg border border-[#34d399]/30 bg-[#34d399]/10 px-2.5 py-1 text-[11.5px] font-medium text-[#34d399]"
        >
          <Check size={12} /> Applied — port freed
        </motion.div>
      )}
    </>
  );
}

/* ── Agent scene ─────────────────────────────────────────────────────────── */

function AgentScene({
  prompt,
  showTask,
  showThinking,
  thinking,
  showEdit,
  diffCount,
  showRun,
  showMessage,
  done,
}: {
  prompt: string;
  showTask: boolean;
  showThinking: boolean;
  thinking: boolean;
  showEdit: boolean;
  diffCount: number;
  showRun: boolean;
  showMessage: boolean;
  done: boolean;
}) {
  return (
    <AnimatePresence>
      {showTask && (
        <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.3 }}>
          <Block
            accent={done ? "ok" : "run"}
            header={prompt}
            meta={done ? <Meta time="21s" ok /> : <span className="text-[11px] text-white/30">running</span>}
          >
            {showThinking && <ThinkingRow active={thinking} />}
            {showThinking && !thinking && (
              <Activity icon={FileSearch} label="Explored 1 file" time="0.4s" />
            )}

            {showEdit && (
              <div>
                <Activity icon={FilePen} label="Edit File  queue.test.ts" time="1.0s" accent />
                <div className="ml-5 mt-1.5 overflow-hidden rounded-lg border border-white/10 bg-black/40">
                  <div className="flex items-center gap-2 border-b border-white/8 px-2.5 py-1 text-[10.5px] text-white/45">
                    queue.test.ts
                    <span className="text-[#34d399]">+9</span>
                    <span className="text-[#f4677a]">-10</span>
                  </div>
                  <div className="px-1 py-1">
                    {DIFF.slice(0, diffCount).map((d, i) => (
                      <div
                        key={i}
                        className={`flex gap-2 px-1.5 text-[11.5px] leading-[1.35] ${
                          d.t === "+"
                            ? "bg-[#34d399]/10 text-[#8ff0c4]"
                            : d.t === "-"
                              ? "bg-[#f4677a]/10 text-[#ffb0bc]"
                              : "text-white/45"
                        }`}
                      >
                        <span className="w-2 shrink-0 text-center opacity-70">
                          {d.t === " " ? "" : d.t}
                        </span>
                        <span className="whitespace-pre">{d.s}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            )}

            {showRun && (
              <Activity
                icon={SquareTerminal}
                label="Run Command  npm run test"
                time="3.1s"
                accent
                trailing={done ? <span className="text-[#34d399]">✓ 12 passed</span> : undefined}
              />
            )}
          </Block>

          {!done && !showMessage && <LoadingLine message={showRun ? "Running tests…" : "Working…"} />}

          {showMessage && (
            <motion.div initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} className="mt-1">
              <div className="mb-1.5 flex items-center gap-2">
                <span className="h-1.5 w-1.5 rounded-full bg-[#4fd4f6]" />
                <span className="font-[family-name:var(--font-outfit)] text-[12px] font-semibold text-[#9be7ff]">
                  Agent
                </span>
              </div>
              <p className="font-[family-name:var(--font-inter)] text-[12.5px] leading-relaxed text-white/70">
                Added a focused test covering{" "}
                <span className="text-white/90">cleanupAppCaches</span> and ran the
                suite — all green. Want me to cover the keep-list edge case too?
              </p>
            </motion.div>
          )}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/* ── Loaders (agent_block_steps.rs) ─────────────────────────────────────── */

/** "✳ Thinking" with a shimmering dot while it runs; "✳ Thought" after. */
function ThinkingRow({ active }: { active: boolean }) {
  return (
    <motion.div
      initial={{ opacity: 0, x: -4 }}
      animate={{ opacity: 1, x: 0 }}
      className="mt-1 flex items-center gap-2 font-medium"
    >
      {active && <span className="demo-shimmer-dot text-[12px]">●</span>}
      <span style={{ color: active ? ACCENT : "rgba(255,255,255,0.35)" }}>
        {active ? "▸ ✳ Thinking" : "▸ ✳ Thought"}
      </span>
      {!active && <span className="text-[12px] font-normal text-white/45">Where the cache cleanup is tested</span>}
    </motion.div>
  );
}

/** The agent's loading line: its message with a light sweeping across it. */
function LoadingLine({ message }: { message: string }) {
  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      className="-mx-4 mb-2 border-t border-white/[0.07] px-4 py-1.5"
    >
      <span className="demo-shimmer text-[12.5px]">{message}</span>
    </motion.div>
  );
}

function Activity({
  icon: Icon,
  label,
  time,
  accent,
  trailing,
}: {
  icon: typeof FilePen;
  label: string;
  time: string;
  accent?: boolean;
  trailing?: React.ReactNode;
}) {
  return (
    <motion.div
      initial={{ opacity: 0, x: -4 }}
      animate={{ opacity: 1, x: 0 }}
      className="mt-1 flex items-center gap-2"
    >
      <ChevronRight size={12} className="text-white/30" />
      <Icon size={12} className={accent ? "text-[#9be7ff]" : "text-white/40"} />
      <span className={accent ? "text-white/80" : "text-white/55"}>{label}</span>
      {trailing && <span className="ml-1 text-[11px]">{trailing}</span>}
      <span className="ml-auto text-[10.5px] text-white/30">{time}</span>
    </motion.div>
  );
}

/* ── Shared block ────────────────────────────────────────────────────────── */

function Block({
  accent,
  header,
  meta,
  children,
}: {
  accent: "ok" | "run" | "err";
  header: string;
  meta: React.ReactNode;
  children: React.ReactNode;
}) {
  const bar = accent === "err" ? "bg-[#f4677a]" : accent === "run" ? "bg-white/40" : "bg-[#28c840]";
  return (
    <div className="mb-3 pl-3">
      <div className="relative">
        <span className={`absolute -left-3 top-0 h-full w-[2px] rounded ${bar}`} />
        <div className="flex items-center gap-2">
          <span className="text-white/40">➜</span>
          <span className="truncate text-white/90">{header}</span>
          <span className="ml-auto shrink-0">{meta}</span>
        </div>
        <div className="mt-1.5 space-y-0.5">{children}</div>
      </div>
    </div>
  );
}

function Meta({ time, ok, err }: { time: string; ok?: boolean; err?: boolean }) {
  return (
    <span className="flex items-center gap-2">
      <span className="text-[11px] text-white/35">{time}</span>
      {ok && (
        <span className="rounded bg-[#28c840]/15 px-1.5 py-0.5 text-[10px] font-medium text-[#34d399]">ok</span>
      )}
      {err && (
        <span className="rounded bg-[#f4677a]/15 px-1.5 py-0.5 text-[10px] font-medium text-[#f4677a]">err</span>
      )}
    </span>
  );
}

/* ── Left sidebar ────────────────────────────────────────────────────────── */

// The app's default dark theme: sidebars sit a step above the content.
const ACCENT = "#75baf2";

export function TrafficLights() {
  return (
    <span className="flex items-center gap-2">
      <span className="h-3 w-3 rounded-full bg-[#ff5f57]" />
      <span className="h-3 w-3 rounded-full bg-[#febc2e]" />
      <span className="h-3 w-3 rounded-full bg-[#28c840]" />
    </span>
  );
}

// The status a workspace's agents are in, as the app's sidebar shows it.
export type WsStatus = "idle" | "working" | "permission" | "review" | "failed";

// The app's fixed status colours (worktree_status.rs).
const WORKING_AMBER = "#d9a441";
const NEEDS_INPUT_ORANGE = "#f08a3c";
const REVIEW_GREEN = "#34d399";
const FAILED_RED = "#f4677a";
const SIDEBAR_BG = "#12151b";

function statusColor(s: WsStatus): string | undefined {
  if (s === "working") return WORKING_AMBER;
  if (s === "permission") return NEEDS_INPUT_ORANGE;
  if (s === "review") return REVIEW_GREEN;
  if (s === "failed") return FAILED_RED;
  return undefined;
}

const NAV = [
  { icon: ListTodo, label: "Tasks" },
  { icon: Clock, label: "Automations" },
  { icon: Layers, label: "Agent Coordination" },
  { icon: GitPullRequest, label: "Code reviews" },
];

type Card = {
  title: string;
  branch: string;
  agents: string[];
  diff?: [number, number];
  status: WsStatus;
  active?: boolean;
};

function LeftSidebar({ status }: { status: WsStatus }) {
  const cards: Card[] = [
    {
      title: "cache-cleanup-tests",
      branch: "feat/cache-cleanup",
      agents: ["dragent", "claude"],
      diff: [155, 0],
      status,
      active: true,
    },
    {
      title: "fix-retry-errors",
      branch: "fix/retry-errors",
      agents: ["codex"],
      diff: [42, 7],
      status: "permission",
    },
    {
      title: "docs-refresh",
      branch: "docs/refresh",
      agents: ["opencode", "gemini"],
      status: "idle",
    },
  ];
  return (
    <aside className="hidden w-[232px] shrink-0 flex-col border-r border-white/[0.07] bg-[#12151b] sm:flex">
      <div className="flex h-9 shrink-0 items-center gap-3.5 border-b border-white/[0.07] px-3">
        <TrafficLights />
        <span className="flex h-6 w-6 items-center justify-center rounded-md bg-white/[0.05] text-white/45">
          <PanelLeft size={13} />
        </span>
      </div>

      {/* top nav (panel_nav.rs: render_sidebar_nav) */}
      <div className="flex flex-col gap-0.5 px-2 pb-1.5 pt-2">
        <div
          className="flex h-[30px] items-center gap-2.5 rounded-md px-2.5"
          style={{ background: `${ACCENT}26` }}
        >
          <span
            className="flex h-[19px] w-[19px] items-center justify-center rounded-[5px] text-[#0c1015]"
            style={{ background: ACCENT }}
          >
            <Plus size={12} strokeWidth={2.5} />
          </span>
          <span className="text-[13px] text-white/90">New Workspace</span>
        </div>
        {NAV.map(({ icon: Icon, label }) => (
          <div key={label} className="flex h-[30px] items-center gap-2.5 rounded-md px-2.5 text-[13px] text-white/85">
            <span className="flex w-[19px] justify-center">
              <Icon size={15} className="text-white/50" />
            </span>
            {label}
          </div>
        ))}
      </div>

      {/* Projects header */}
      <div className="flex shrink-0 items-center border-b border-white/[0.07] px-2 py-1 text-white/50">
        <span className="flex items-center gap-1.5 px-1.5 py-1.5">
          <ChevronDown size={10} />
          <span className="text-[11px] font-medium text-white/85">Projects</span>
        </span>
        <span className="ml-auto flex items-center">
          <span className="p-1.5"><Search size={13} /></span>
          <span className="p-1.5"><ArrowDownWideNarrow size={13} /></span>
          <span className="p-1.5"><FolderPlus size={13} /></span>
        </span>
      </div>

      {/* project groups and their workspace cards */}
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden pt-0.5">
        <ProjectHeader name="orbit-web" count={3} />
        <div className="mt-1">
          {cards.map((c) => (
            <WorkspaceCard key={c.title} card={c} />
          ))}
        </div>
        <ProjectHeader name="atlas-mobile" count={2} />
        <ProjectHeader name="payments-api" count={1} remote />
      </div>

      <div className="flex h-11 shrink-0 items-center gap-2 border-t border-white/[0.07] px-3">
        <span
          className="flex h-5 w-5 items-center justify-center rounded-[5px] text-[10px] font-semibold text-[#0c1015]"
          style={{ background: ACCENT }}
        >
          N
        </span>
        <span className="truncate text-white/70">Northwind Labs</span>
        <span className="ml-auto flex items-center gap-2.5 text-white/40">
          <CircleHelp size={13} />
          <LogOut size={13} />
        </span>
      </div>
    </aside>
  );
}

/** A project's heading: thumbnail (or a server for remote projects), name, + and count. */
function ProjectHeader({ name, count, remote }: { name: string; count: number; remote?: boolean }) {
  return (
    <div className="mx-1 mt-1 flex items-center gap-[7px] rounded pb-[7px] pl-1 pr-2.5 pt-[7px]">
      {remote ? (
        <Server size={12} className="mx-0.5 shrink-0" style={{ color: ACCENT }} />
      ) : (
        <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-[5px] border border-white/10 bg-white/[0.06] text-[9.5px] font-semibold text-white/55">
          {name[0].toUpperCase()}
        </span>
      )}
      <span className="truncate text-[14px] font-medium text-white/90">{name}</span>
      {remote && <span className="h-[7px] w-[7px] shrink-0 rounded-full bg-[#34d399]" />}
      <span className="ml-auto flex shrink-0 items-center gap-1 text-white/45">
        <span className="p-[5px]"><Plus size={12} /></span>
        <span className="text-[9.5px] text-white/30">{count}</span>
      </span>
    </div>
  );
}

/** A workspace card (panel_coordination.rs: workspace_card). */
function WorkspaceCard({ card }: { card: Card }) {
  const { title, branch, agents, diff, status, active } = card;
  return (
    <div
      className="mb-1.5 ml-3 mr-2 rounded-lg px-2 py-[7px]"
      style={{ background: active ? `${ACCENT}1f` : "rgba(255,255,255,0.035)" }}
    >
      <div className="flex items-center gap-2">
        <WorkspaceTile status={status} active={active} />
        <span className="truncate text-[13px] text-white/90">{title}</span>
      </div>
      <div className="mt-1 flex items-center gap-[5px] pl-[21px] pt-0.5">
        <AgentPile agents={agents} />
        <GitBranch size={10} className="ml-[3px] shrink-0 text-white/30" />
        <span className="min-w-0 flex-1 truncate text-[11px] text-white/50">{branch}</span>
        {diff && diff[0] + diff[1] > 0 && (
          <span className="flex shrink-0 gap-[3px] text-[10.5px]">
            <span style={{ color: REVIEW_GREEN }}>+{diff[0]}</span>
            <span style={{ color: FAILED_RED }}>−{diff[1]}</span>
          </span>
        )}
      </div>
    </div>
  );
}

const AVATAR = 17;
const STACK_STEP = 12;

/** Round agent logos, stacked like avatars, the first on top. */
function AgentPile({ agents }: { agents: string[] }) {
  const shown = agents.slice(0, 3);
  return (
    <span
      className="relative shrink-0"
      style={{ width: AVATAR + STACK_STEP * (shown.length - 1), height: AVATAR }}
    >
      {shown.map((id, i) => (
        <span
          key={id}
          className="absolute top-0 flex items-center justify-center rounded-full bg-[#1f232b]"
          style={{
            left: i * STACK_STEP,
            width: AVATAR + 3,
            height: AVATAR + 3,
            margin: -1.5,
            border: `1.5px solid ${SIDEBAR_BG}`,
            zIndex: shown.length - i,
          }}
        >
          <Image
            src={id === "dragent" ? "/dragent-mark.png" : `/agents/${id}.svg`}
            alt=""
            width={11}
            height={11}
            className={agentMarkClass(id)}
          />
        </span>
      ))}
    </span>
  );
}

const TILE = 19;

/**
 * The workspace's Git tile (workspace_activity.rs): while its agents work a
 * light laps the border and the corner mark spins; otherwise the corner
 * carries the status dot (none when idle).
 */
export function WorkspaceTile({ status, active }: { status: WsStatus; active?: boolean }) {
  const reduced = !!useReducedMotion();
  const working = status === "working";
  const mark = statusColor(status);
  const tint = working || active ? ACCENT : "rgba(255,255,255,0.85)";
  return (
    <span
      className="relative shrink-0"
      style={{
        width: TILE,
        height: TILE,
        borderRadius: 5,
        background: active ? `${ACCENT}29` : "rgba(255,255,255,0.06)",
        boxShadow: working ? `inset 0 0 0 0.7px ${ACCENT}26` : undefined,
      }}
    >
      {working && (
        <motion.span
          className="absolute left-[6px] top-[5.5px] h-2 w-[7px] rounded"
          style={{ background: `${ACCENT}1c` }}
          animate={reduced ? undefined : { boxShadow: [`0 0 4px 1px ${ACCENT}26`, `0 0 4px 1px ${ACCENT}56`, `0 0 4px 1px ${ACCENT}26`] }}
          transition={{ duration: 2.4, repeat: Infinity, ease: "easeInOut" }}
        />
      )}
      <span className="absolute inset-0 flex items-center justify-center">
        <GitBranch size={10} style={{ color: tint }} />
      </span>
      {working && !reduced && <TileLap />}
      {mark && (
        <span
          className="absolute flex items-center justify-center rounded-full"
          style={{
            width: 10.5,
            height: 10.5,
            left: TILE - 10.5 + 3,
            top: TILE - 10.5 + 3,
            background: SIDEBAR_BG,
            border: "1px solid rgba(255,255,255,0.1)",
          }}
        >
          {working ? (
            <motion.svg
              width="8"
              height="8"
              viewBox="0 0 8 8"
              animate={reduced ? undefined : { rotate: 360 }}
              transition={{ duration: 0.9, repeat: Infinity, ease: "linear" }}
            >
              <circle
                cx="4"
                cy="4"
                r="2.9"
                fill="none"
                stroke={mark}
                strokeWidth="1.7"
                strokeLinecap="round"
                pathLength={100}
                strokeDasharray="75 25"
              />
            </motion.svg>
          ) : (
            <span className="h-1.5 w-1.5 rounded-full" style={{ background: mark }} />
          )}
        </span>
      )}
    </span>
  );
}

/** A light lapping the tile's border clockwise, with a fading tail. */
function TileLap() {
  // Stacked dashes share one leading tip; shorter ones are brighter.
  const tail = [
    { len: 28, alpha: 0.18 },
    { len: 20, alpha: 0.3 },
    { len: 12, alpha: 0.55 },
    { len: 5, alpha: 1 },
  ];
  const inset = 0.85;
  return (
    <svg
      className="pointer-events-none absolute inset-0"
      width={TILE}
      height={TILE}
      viewBox={`0 0 ${TILE} ${TILE}`}
    >
      {tail.map(({ len, alpha }) => (
        <motion.rect
          key={len}
          x={inset}
          y={inset}
          width={TILE - inset * 2}
          height={TILE - inset * 2}
          rx={5 - inset}
          fill="none"
          stroke={ACCENT}
          strokeOpacity={alpha}
          strokeWidth={1.3}
          strokeLinecap="round"
          pathLength={100}
          strokeDasharray={`${len} ${100 - len}`}
          initial={{ strokeDashoffset: len }}
          animate={{ strokeDashoffset: [len, len - 100] }}
          transition={{ duration: 2.4, repeat: Infinity, ease: "linear" }}
        />
      ))}
    </svg>
  );
}

/* ── Agent rail ──────────────────────────────────────────────────────────── */

// Same order as the app; agents that are not installed are dimmed and badged.
const RAIL: { id: string; file?: string; installed?: boolean }[] = [
  { id: "dragent", file: "/dragent-mark.png", installed: true },
  { id: "claude", installed: true },
  { id: "codex", installed: true },
  { id: "gemini", installed: true },
  { id: "opencode", installed: true },
  { id: "amp" },
  { id: "copilot", installed: true },
  { id: "antigravity" },
  { id: "auggie" },
  { id: "autohand" },
  { id: "charm", file: "charm.png" },
  { id: "cline", file: "cline.png" },
  { id: "codebuddy" },
  { id: "codebuff", file: "codebuff.png" },
  { id: "commandcode" },
  { id: "continue", file: "continue.png" },
  { id: "cursor", installed: true },
  { id: "devin" },
  { id: "droid" },
  { id: "freebuff", file: "freebuff.png" },
];

function AgentRail() {
  return (
    <aside className="hidden w-9 shrink-0 flex-col items-center border-l border-white/[0.07] bg-[#12151b] lg:flex">
      <div className="flex h-9 w-full shrink-0 items-center justify-center border-b border-white/[0.07] text-white/40">
        <ArrowLeft size={13} />
      </div>
      <div className="flex min-h-0 flex-col items-center gap-[15px] overflow-hidden pt-3">
        {RAIL.map((agent) => (
          <span key={agent.id} className="relative h-4 w-4 shrink-0">
            <Image
              src={agent.file?.startsWith("/") ? agent.file : `/agents/${agent.file ?? `${agent.id}.svg`}`}
              alt=""
              width={16}
              height={16}
              className={[agentMarkClass(agent.id), agent.installed ? undefined : "opacity-45"]
                .filter(Boolean)
                .join(" ") || undefined}
            />
            {!agent.installed && (
              <span className="absolute -bottom-1.5 -right-2 flex h-3 w-3 items-center justify-center rounded-full bg-[#12151b] text-white/80">
                <ArrowDownToLine size={9} strokeWidth={2.5} />
              </span>
            )}
          </span>
        ))}
      </div>
    </aside>
  );
}

/* ── Right sidebar ───────────────────────────────────────────────────────── */

const TREE: { name: string; dir?: boolean; state?: "modified" | "ignored" }[] = [
  { name: ".github", dir: true },
  { name: "app", dir: true, state: "modified" },
  { name: "lib", dir: true },
  { name: "public", dir: true },
  { name: "tests", dir: true, state: "modified" },
  { name: "node_modules", dir: true, state: "ignored" },
  { name: "target", dir: true, state: "ignored" },
  { name: ".gitignore" },
  { name: "package.json" },
  { name: "README.md" },
];

function FileGlyph({ name }: { name: string }) {
  if (name === ".gitignore") return <GitBranch size={12} className="text-[#f05033]" />;
  if (name.endsWith(".json")) return <Braces size={12} className="text-[#e2c541]" />;
  return <FileText size={12} className="text-[#6fb3f2]" />;
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-white/35">
      {children}
    </span>
  );
}

function RightSidebar({ hasError }: { hasError: boolean }) {
  return (
    <aside className="hidden w-[248px] shrink-0 flex-col border-l border-white/[0.07] bg-[#12151b] lg:flex">
      {/* header: panel toggle · branch diff · review */}
      <div className="flex h-9 shrink-0 items-stretch border-b border-white/[0.07] text-[11.5px]">
        <span className="flex w-9 items-center justify-center border-r border-white/[0.07] text-white/45">
          <PanelLeft size={13} />
        </span>
        <span className="flex items-center gap-1.5 border-r border-white/[0.07] px-2.5 font-[family-name:var(--font-mono-jb)]">
          <GitCompareArrows size={12} style={{ color: ACCENT }} />
          <span className="text-[#34d399]">+155</span>
          <span className="text-[#f4677a]">-0</span>
        </span>
        <span className="flex items-center gap-1.5 px-2.5 text-white/60">
          <GitPullRequest size={12} className="text-white/45" />
          Review
        </span>
        <span className="ml-auto flex items-center border-l border-white/[0.07] px-1.5 text-white/40">
          <ChevronDown size={12} />
        </span>
      </div>

      {/* tabs */}
      <div className="flex h-9 shrink-0 items-center border-b border-white/[0.07] px-1.5 text-[11.5px]">
        <span className="flex items-center gap-1.5 rounded-md bg-white/[0.07] px-[7px] py-1 text-white/90">
          <Folder size={12} />
          Files
        </span>
        <span className="flex items-center gap-1.5 px-[7px] py-1 text-white/50">
          <Search size={12} />
          Search
        </span>
        <span className="flex items-center gap-1.5 px-[7px] py-1 text-white/50">
          <GitBranch size={12} />
          Changes
          <span className="rounded bg-white/[0.08] px-1 text-[10px] text-white/55">3</span>
        </span>
      </div>

      {/* project + branch */}
      <div className="shrink-0 border-b px-3 py-3" style={{ borderColor: `${ACCENT}33` }}>
        <SectionLabel>Project</SectionLabel>
        <div className="mt-2 flex items-center gap-2.5">
          <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-white/10 bg-white/[0.04] text-[12px] font-semibold text-white/85">
            O
          </span>
          <div className="min-w-0">
            <div className="truncate font-semibold text-white/90">orbit-web</div>
            <div className="truncate text-[10.5px] text-white/40">~/dev/orbit-web</div>
          </div>
        </div>
        <div className="mt-2.5 flex items-center gap-1.5 text-[11px] text-white/45">
          Branch
          <GitBranch size={11} className="text-[#b9a3ff]" />
          <span className="truncate font-[family-name:var(--font-mono-jb)] text-[#b9a3ff]">feat/cache-cleanup</span>
        </div>
      </div>

      {/* files */}
      <div className="flex h-8 shrink-0 items-center border-b border-white/[0.07] px-3">
        <SectionLabel>Files</SectionLabel>
        <span className="ml-auto flex items-center gap-2.5 text-white/40">
          <FilePlus size={12} />
          <FolderPlus size={12} />
          <RefreshCw size={12} />
        </span>
      </div>
      <div className="min-h-0 flex-1 overflow-hidden py-1 font-[family-name:var(--font-mono-jb)] text-[11.5px]">
        {TREE.map((f) => (
          <div key={f.name} className="flex h-6 items-center gap-1.5 pl-2.5 pr-2">
            {f.dir ? (
              <ChevronRight size={10} className="shrink-0 text-white/30" />
            ) : (
              <span className="w-2.5 shrink-0" />
            )}
            {f.dir ? (
              <Folder size={12} className="shrink-0 text-white/45" />
            ) : (
              <FileGlyph name={f.name} />
            )}
            <span
              className={`truncate ${
                f.state === "modified"
                  ? "text-[#e5c07b]"
                  : f.state === "ignored"
                    ? "text-white/30"
                    : "text-white/75"
              }`}
            >
              {f.name}
            </span>
            {f.state === "modified" && (
              <span className="ml-auto h-3.5 w-[3px] shrink-0 rounded-full bg-[#e5c07b]" />
            )}
          </div>
        ))}
      </div>

      {/* errors */}
      <div className="shrink-0 border-t border-white/[0.07] px-3 py-2.5">
        <SectionLabel>Errors</SectionLabel>
        <AnimatePresence mode="wait">
          {hasError ? (
            <motion.div
              key="err"
              initial={{ opacity: 0, y: -4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0 }}
              className="mt-1.5 flex items-center gap-1.5 text-[11.5px] text-[#f4677a]"
            >
              <span className="h-1.5 w-1.5 rounded-full bg-[#f4677a]" />
              1 error — port in use
            </motion.div>
          ) : (
            <motion.div
              key="ok"
              initial={{ opacity: 0, y: -4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0 }}
              className="mt-1.5 text-[11.5px] text-white/40"
            >
              No errors
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </aside>
  );
}

/* ── Cursor ──────────────────────────────────────────────────────────────── */

function cursorPos(mode: Mode, key: string): { x: number; y: number } {
  // The shell input bar has no toolbar row, so its prompt sits lower.
  if (key === "typing" || key === "idle" || key === "submit")
    return { x: 22, y: mode === "shell" ? 93 : 88 };
  if (mode === "shell" && key === "diagnose") return { x: 30, y: 47 };
  if (mode === "shell" && key === "fixing") return { x: 13, y: 54 };
  if (mode === "shell" && key === "resolved") return { x: 62, y: 50 };
  if (mode === "agent" && (key === "editing" || key === "running")) return { x: 55, y: 52 };
  return { x: 50, y: 45 };
}

function Cursor({ clicking }: { clicking: boolean }) {
  return (
    <div className="relative">
      <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
        <path d="M5 3l6 16 2.5-6.5L20 10 5 3z" fill="white" stroke="#05070a" strokeWidth="1.3" strokeLinejoin="round" />
      </svg>
      {clicking && (
        <motion.span
          initial={{ scale: 0, opacity: 0.7 }}
          animate={{ scale: 2.4, opacity: 0 }}
          transition={{ duration: 0.5 }}
          className="absolute -left-1 -top-1 h-6 w-6 rounded-full border border-white/70"
        />
      )}
    </div>
  );
}
