"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import Image from "next/image";
import { LayoutGroup, motion, useReducedMotion } from "framer-motion";
import {
  ArrowUp,
  Check,
  ChevronDown,
  Clock,
  Crown,
  Layers,
  ListTodo,
  Mic,
  Monitor,
  Folder,
  GitBranch,
  GitPullRequest,
  MessageSquare,
  Pause,
  Play,
  Plus,
  SquareTerminal,
  TriangleAlert,
} from "lucide-react";
import { TrafficLights, WorkspaceTile, type WsStatus } from "./AppDemo";
import { HeroGlyphField } from "./HeroGlyphField";

/* ── The hero's team demo ─────────────────────────────────────────────────
   A self-playing recreation of Dragent running an agent team, in the app's
   own UI: the New workspace screen with Team on, the team gathering, the
   lead writing the plan, cards moving across the Board by themselves, the
   lead asking the person one question, the check, and the finished change.
   The loaders are the app's: a light lapping a box's edge, faces glowing in
   turn. All live DOM, so it stays crisp at any size. ──────────────────── */

type Agent = "claude" | "codex" | "opencode" | "gemini";
type Column = "next" | "working" | "review" | "needs" | "done";
type Stage =
  | "compose"
  | "starting"
  | "planning"
  | "planned"
  | "working"
  | "term1"
  | "term2"
  | "needs"
  | "checking"
  | "ready";
type View = "board" | "t1" | "t2";

const GOAL =
  "Move checkout to the new payments API. Keep old mobile clients working for 30 days, and add tests for refunds.";

const STAGES: { stage: Stage; ms: number }[] = [
  { stage: "compose", ms: 5600 },
  { stage: "starting", ms: 2400 },
  { stage: "planning", ms: 2800 },
  { stage: "planned", ms: 1800 },
  { stage: "working", ms: 3600 },
  { stage: "term1", ms: 3900 },
  { stage: "term2", ms: 3500 },
  { stage: "needs", ms: 3800 },
  { stage: "checking", ms: 3200 },
  { stage: "ready", ms: 4200 },
];

const CHAPTERS: { label: string; from: Stage }[] = [
  { label: "Give the goal", from: "compose" },
  { label: "The lead plans", from: "planning" },
  { label: "The team builds", from: "working" },
  { label: "Look inside", from: "term1" },
  { label: "It needs you", from: "needs" },
  { label: "It's checked", from: "checking" },
  { label: "Ready for review", from: "ready" },
];

const TASKS: { id: string; title: string; agent: Agent; model: string; qa?: boolean }[] = [
  { id: "t1", title: "New payments client", agent: "codex", model: "gpt-6" },
  { id: "t2", title: "Keep old clients working", agent: "opencode", model: "big-pickle" },
  { id: "t3", title: "Refund tests, then check it all", agent: "gemini", model: "gemini-3", qa: true },
];

function columnOf(task: string, stage: Stage): Column {
  const order: Stage[] = STAGES.map((s) => s.stage);
  const at = order.indexOf(stage);
  const after = (s: Stage) => at >= order.indexOf(s);
  if (task === "t3") {
    if (after("ready")) return "done";
    if (after("checking")) return "working";
    return "next";
  }
  if (after("checking")) return "done";
  if (after("working")) return "working";
  return "next";
}

function stageIndex(stage: Stage) {
  return STAGES.findIndex((s) => s.stage === stage);
}

/** What the main area shows: the Board, or a worker's terminal tab. */
function viewOf(stage: Stage): View {
  return stage === "term1" ? "t1" : stage === "term2" ? "t2" : "board";
}

/* The pointer's script: where it goes in each stage, and what its click
   does. A click lands just before the stage ends, so the next stage is what
   the click opened. Targets are elements marked data-cursor="…". */
type Step = { at: number; to: string; press?: boolean; act?: "team" | "answer" };
const POINTER: Partial<Record<Stage, Step[]>> = {
  compose: [
    { at: 2900, to: "coordinate" },
    { at: 3600, to: "coordinate", press: true, act: "team" },
    { at: 4500, to: "start-team" },
    { at: 5150, to: "start-team", press: true },
  ],
  working: [
    { at: 1900, to: "term-t1" },
    { at: 3050, to: "term-t1", press: true },
  ],
  term1: [
    { at: 2300, to: "side-t2" },
    { at: 3300, to: "side-t2", press: true },
  ],
  term2: [
    { at: 2100, to: "tab-board" },
    { at: 2950, to: "tab-board", press: true },
  ],
  needs: [
    { at: 1500, to: "answer" },
    { at: 2500, to: "answer", press: true, act: "answer" },
  ],
  ready: [
    { at: 1700, to: "review" },
    { at: 2800, to: "review", press: true },
  ],
};

export function HeroTeamDemo() {
  const reduced = !!useReducedMotion();
  const [index, setIndex] = useState(0);
  const [typed, setTyped] = useState(0);
  const [playing, setPlaying] = useState(true);
  const [teamOn, setTeamOn] = useState(false);
  const [answered, setAnswered] = useState(false);
  const [pointer, setPointer] = useState({ x: 620, y: 360, shown: false, press: 0 });
  const frame = useRef<HTMLDivElement>(null);
  const stage = STAGES[index].stage;

  // Puts the pointer on a target's centre, if it's on screen.
  const pointAt = useCallback((target: string, press: boolean) => {
    const root = frame.current;
    const el = root?.querySelector<HTMLElement>(`[data-cursor="${target}"]`);
    if (!root || !el) return;
    const box = el.getBoundingClientRect();
    const outer = root.getBoundingClientRect();
    if (box.width === 0 || box.height === 0) return;
    setPointer((p) => ({
      x: box.left - outer.left + box.width / 2,
      y: box.top - outer.top + box.height / 2,
      shown: true,
      press: press ? p.press + 1 : p.press,
    }));
  }, []);

  // Advance through the stages; the compose stage types the goal, then turns Team on.
  useEffect(() => {
    if (!playing) return;
    const timers: ReturnType<typeof setTimeout>[] = [];
    // Moving on; back at the start, the box is empty and Team off again.
    const advance = () => {
      const next = (index + 1) % STAGES.length;
      if (next === 0) {
        setTyped(0);
        setTeamOn(false);
        setPointer((p) => ({ ...p, shown: false }));
      }
      setAnswered(false);
      setIndex(next);
    };
    // The pointer moves and clicks; its clicks do what they show.
    (POINTER[stage] ?? []).forEach((step) => {
      timers.push(
        setTimeout(() => {
          if (!reduced) pointAt(step.to, !!step.press);
          if (step.act === "team") setTeamOn(true);
          if (step.act === "answer") setAnswered(true);
        }, step.at),
      );
    });
    if (stage === "compose") {
      const step = reduced ? GOAL.length : 3;
      let n = 0;
      const tick = setInterval(() => {
        n = Math.min(GOAL.length, n + step);
        setTyped(n);
        if (n >= GOAL.length) clearInterval(tick);
      }, 28);
      timers.push(setTimeout(() => clearInterval(tick), STAGES[index].ms));
      timers.push(setTimeout(advance, STAGES[index].ms));
      return () => {
        clearInterval(tick);
        timers.forEach(clearTimeout);
      };
    }
    timers.push(setTimeout(advance, STAGES[index].ms));
    return () => timers.forEach(clearTimeout);
  }, [index, playing, stage, reduced, pointAt]);

  const jump = (from: Stage) => {
    const at = stageIndex(from);
    setIndex(at);
    setAnswered(false);
    if (from !== "compose") {
      setTyped(GOAL.length);
      setTeamOn(true);
    } else {
      setTyped(0);
      setTeamOn(false);
    }
  };

  const chapter = CHAPTERS.reduce((current, c, i) => (stageIndex(c.from) <= index ? i : current), 0);
  const onBoard = stageIndex(stage) >= stageIndex("planning");

  return (
    <div className="hero-team-demo product-demo relative mx-auto mt-14 w-full max-w-[1240px] text-left">
      <div ref={frame} className="relative overflow-hidden rounded-2xl border border-white/10 bg-[#0c1015] shadow-[0_40px_120px_-30px_rgba(0,0,0,0.9)]">
        <div className="flex h-9 items-center gap-3 border-b border-white/[0.07] bg-[#0f1318] px-3.5">
          <TrafficLights />
          <span className="ml-3 truncate text-[12px] text-white/45">
            {onBoard ? "Acme Web · Billing API migration" : "Acme Web · New workspace"}
          </span>
        </div>
        <div className="flex h-[600px] text-[12.5px] sm:h-[680px]">
          <Sidebar stage={stage} />
          <div className="relative min-w-0 flex-1">
            {onBoard ? <Main stage={stage} answered={answered} /> : (
              <Compose typed={typed} teamOn={teamOn} starting={stage === "starting"} />
            )}
          </div>
        </div>
        {!reduced && <Pointer {...pointer} />}
      </div>

      <div className="mt-5 flex flex-wrap items-center justify-center gap-1.5">
        <button
          type="button"
          onClick={() => setPlaying((p) => !p)}
          aria-label={playing ? "Pause the demo" : "Play the demo"}
          className="mr-1 inline-flex h-8 w-8 items-center justify-center rounded-full border border-white/12 text-white/70 hover:text-white"
        >
          {playing ? <Pause size={13} /> : <Play size={13} className="ml-0.5" />}
        </button>
        {CHAPTERS.map((c, i) => (
          <button
            key={c.label}
            type="button"
            onClick={() => jump(c.from)}
            className={`rounded-full px-3 py-1.5 text-[12px] transition-colors ${
              i === chapter ? "bg-white/10 text-white" : "text-white/45 hover:text-white/80"
            }`}
          >
            <span className="mr-1.5 font-[family-name:var(--font-mono-jb)] text-[10.5px] text-white/35">0{i + 1}</span>
            {c.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/* ── Pieces ─────────────────────────────────────────────────────────────── */

function AgentFace({ agent, size = 18, pulse }: { agent: Agent; size?: number; pulse?: number }) {
  return (
    <span
      className={`relative inline-flex shrink-0 items-center justify-center rounded-full bg-[#1b2128] ${pulse !== undefined ? "face-pulse" : ""}`}
      style={{ width: size, height: size, animationDelay: pulse !== undefined ? `${pulse * 0.7}s` : undefined }}
    >
      <Image src={`/agents/${agent}.svg`} alt="" width={size - 6} height={size - 6} className={agent === "codex" || agent === "opencode" ? "agent-mark-dark" : undefined} />
    </span>
  );
}

function Sidebar({ stage }: { stage: Stage }) {
  const at = stageIndex(stage);
  const team = at >= stageIndex("starting");
  const workers = at >= stageIndex("working");
  const needs = stage === "needs";
  const ready = stage === "ready";
  const leadStatus: WsStatus = needs ? "permission" : ready ? "review" : team ? "working" : "idle";
  const workerStatus = (task: string): WsStatus =>
    ready ? "review" : columnOf(task, stage) === "done" ? "review" : "working";
  return (
    <aside className="hidden w-[232px] shrink-0 flex-col border-r border-white/[0.07] bg-[#12151b] p-2 text-[12.5px] md:flex">
      <span className="flex items-center gap-2.5 rounded-md bg-[#75baf2]/[0.08] px-2 py-1.5 text-white/90">
        <span className="flex h-5 w-5 items-center justify-center rounded-md bg-[#75baf2] text-[#0b1a26]">
          <Plus size={13} />
        </span>
        New Workspace
      </span>
      {[
        { icon: ListTodo, label: "Tasks" },
        { icon: Clock, label: "Automations" },
        { icon: Layers, label: "Agent Coordination", badge: needs },
        { icon: GitPullRequest, label: "Code reviews" },
      ].map(({ icon: Icon, label, badge }) => (
        <span key={label} className="flex items-center gap-2.5 px-2 py-1.5 text-white/55">
          <Icon size={14} className="text-white/45" />
          {label}
          {badge && <span className="ml-auto rounded-full bg-[#f08a3c] px-1.5 text-[10px] font-medium text-[#1d0d02]">1</span>}
        </span>
      ))}
      <span className="mt-2 border-t border-white/[0.07] px-2 pb-1 pt-3 text-[11px] font-medium text-white/80">Projects</span>
      <span className="flex items-center gap-2 px-2 py-1.5 font-medium text-white/90">
        <span className="flex h-[18px] w-[18px] items-center justify-center rounded bg-white/10 text-[10px] text-white/60">A</span>
        Acme Web
      </span>
      {team && (
        <div className="flex flex-col">
          <span className="flex items-center gap-2 px-2 pb-1 pt-1.5 pl-4 text-[12px] font-semibold text-white/90">
            <Layers size={11} style={{ color: needs ? "#f08a3c" : ready ? "#34d399" : "#d9a441" }} />
            <span className="truncate">Billing API migration</span>
            <span className="ml-auto shrink-0 whitespace-nowrap text-[10.5px] font-medium" style={{ color: needs ? "#f08a3c" : ready ? "#34d399" : "rgba(255,255,255,0.45)" }}>
              {needs ? "needs you" : ready ? "3/3" : stage === "planning" ? "planning" : `${TASKS.filter((t) => columnOf(t.id, stage) === "done").length}/3`}
            </span>
          </span>
          <SideCard title="Lead · plans the migration" status={leadStatus} agent="claude" branch="main" active={viewOf(stage) === "board"} />
          {workers &&
            TASKS.map((t) => (
              <SideCard
                key={t.id}
                target={`side-${t.id}`}
                title={t.title}
                status={workerStatus(t.id)}
                agent={t.agent}
                branch={`${t.id}-billing`}
                active={viewOf(stage) === t.id}
              />
            ))}
        </div>
      )}
      <span className="mt-1 flex items-center gap-2 px-2 py-1.5 font-medium text-white/90">
        <span className="flex h-[18px] w-[18px] items-center justify-center rounded bg-white/10 text-[10px] text-white/60">A</span>
        Acme API
      </span>
    </aside>
  );
}

function SideCard({ title, status, agent, branch, active, target }: { title: string; status: WsStatus; agent: Agent; branch: string; active?: boolean; target?: string }) {
  return (
    <span data-cursor={target} className={`mx-1 mb-1.5 ml-3 flex flex-col rounded-lg px-2 py-1.5 transition-colors duration-300 ${active ? "bg-[#75baf2]/[0.10]" : "bg-white/[0.035]"}`}>
      <span className="flex items-center gap-2 text-[12px] text-white/85">
        <WorkspaceTile status={status} active={active} />
        <span className="truncate">{title}</span>
      </span>
      <span className="mt-1 flex items-center gap-1.5 pl-[27px] font-[family-name:var(--font-mono-jb)] text-[10px] text-white/40">
        <AgentFace agent={agent} size={15} />
        <GitBranch size={9} />
        <span className="truncate">{branch}</span>
      </span>
    </span>
  );
}

/* ── New workspace, with Team ───────────────────────────────────────────── */

function Compose({ typed, teamOn, starting }: { typed: number; teamOn: boolean; starting: boolean }) {
  return (
    <div className="relative flex h-full flex-col items-center justify-center overflow-hidden px-6 sm:px-14">
      {/* The app's moving code backdrop, edge to edge behind the prompt. */}
      <div aria-hidden className="pointer-events-none absolute inset-0">
        <HeroGlyphField full />
      </div>
      <Image src="/dragent-mark.png" alt="" width={34} height={34} className="relative mb-4" />
      <p className="relative mb-3 text-[24px] font-semibold tracking-[-0.02em] text-white/90">What are we working on?</p>
      <div className="relative mb-5 flex flex-wrap justify-center gap-4 text-[12.5px] text-white/50">
        <span className="flex items-center gap-1.5"><Monitor size={12} /> This Mac</span>
        <span className="flex items-center gap-1.5"><Folder size={12} /> Acme Web</span>
        <span className="flex items-center gap-1.5"><GitBranch size={12} /> New worktree</span>
        <span className="flex items-center gap-1.5"><GitBranch size={12} /> main</span>
      </div>
      <div className={`relative w-full max-w-[640px] rounded-xl border bg-[#111418] px-3.5 pb-3 pt-3 ${starting ? "edge-lap border-white/10" : "border-white/12"}`}>
        <p className="min-h-[52px] px-1 text-[14px] leading-relaxed text-white/90">
          <span className="mr-2 text-[#8fd18a]">›</span>
          {GOAL.slice(0, typed)}
          {!starting && <span className="ml-px inline-block h-4 w-[2px] translate-y-[3px] animate-pulse bg-[#75baf2]" />}
        </p>
        <div className="mt-3 flex flex-wrap items-center gap-1.5 font-[family-name:var(--font-mono-jb)] text-[11.5px]">
          <span className="flex h-7 w-7 items-center justify-center text-white/55"><Plus size={14} /></span>
          <span className="relative flex h-7 items-center gap-1.5 rounded-md bg-white/[0.07] px-2 text-[#75baf2]">
            {teamOn && (
              <motion.span initial={{ scale: 0 }} animate={{ scale: 1 }} className="absolute -left-1.5 -top-2 text-[#d9a441]">
                <Crown size={10} />
              </motion.span>
            )}
            <AgentFace agent="claude" size={16} />
            Claude Code <ChevronDown size={11} className="opacity-60" />
          </span>
          <motion.span
            layout
            data-cursor="coordinate"
            className={`flex h-7 items-center gap-1.5 rounded-md px-2 ${teamOn ? "bg-[#75baf2]/[0.14] text-[#75baf2]" : "text-white/55"}`}
          >
            <Layers size={13} />
            {teamOn ? "4 agents" : "Coordinate"}
            {teamOn && (
              <span className="ml-0.5 flex">
                {(["codex", "opencode", "gemini"] as Agent[]).map((a, i) => (
                  <span key={a} className={i ? "-ml-1.5" : undefined}>
                    <AgentFace agent={a} size={18} pulse={starting ? i : undefined} />
                  </span>
                ))}
              </span>
            )}
          </motion.span>
          <span className="flex h-7 items-center gap-1 px-2 text-white/50">Opus <ChevronDown size={11} className="opacity-60" /></span>
          <span className="ml-auto flex h-7 w-7 items-center justify-center text-white/45"><Mic size={14} /></span>
          <span data-cursor="start-team" className={`flex h-8 items-center gap-1.5 rounded-lg px-2.5 font-[family-name:var(--font-sans)] text-[12.5px] font-semibold ${starting ? "bg-white/10 text-white/60" : "bg-[#75baf2] text-[#0b1a26]"}`}>
            {starting ? (
              <span className="flex gap-1">
                {["#d97757", "#e6e6e6", "#8a96ff", "#62aaff"].map((c, i) => (
                  <span key={c} className="bead h-1.5 w-1.5 rounded-full" style={{ background: c, animationDelay: `${i * 0.15}s` }} />
                ))}
              </span>
            ) : teamOn ? (
              <>Start team <ArrowUp size={13} /></>
            ) : (
              <ArrowUp size={14} />
            )}
          </span>
        </div>
      </div>
      <p className="relative mt-4 h-4 text-center text-[11.5px] text-white/40">
        {starting
          ? "Making worktrees for Codex, OpenCode and Gemini…"
          : teamOn
            ? "Claude Code leads and splits the work across 4 agents. You're asked only when it needs you."
            : ""}
      </p>
    </div>
  );
}

/* ── The team's Board ───────────────────────────────────────────────────── */

const COLUMNS: { id: Column; label: string; color: string }[] = [
  { id: "next", label: "Up next", color: "rgba(255,255,255,0.4)" },
  { id: "working", label: "Working", color: "#d9a441" },
  { id: "review", label: "In review", color: "#75baf2" },
  { id: "needs", label: "Needs you", color: "#f08a3c" },
  { id: "done", label: "Done", color: "#34d399" },
];

/* ── The workspace's tabs: the team's Board, and terminals opened from it ── */

function Main({ stage, answered }: { stage: Stage; answered: boolean }) {
  const view = viewOf(stage);
  const at = stageIndex(stage);
  const tabs: { id: View; label: string; agent?: Agent }[] = [{ id: "board", label: "Billing API migration" }];
  if (at >= stageIndex("term1")) tabs.push({ id: "t1", label: "Codex · t1", agent: "codex" });
  if (at >= stageIndex("term2")) tabs.push({ id: "t2", label: "OpenCode · t2", agent: "opencode" });
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-9 shrink-0 items-end gap-0.5 border-b border-white/[0.07] bg-[#0f1318] px-2">
        {tabs.map((t) => (
          <span
            key={t.id}
            data-cursor={t.id === "board" ? "tab-board" : undefined}
            className={`flex h-[30px] items-center gap-1.5 rounded-t-md px-3 text-[11.5px] ${
              view === t.id ? "bg-[#0c1015] text-white/90" : "text-white/45"
            }`}
          >
            {t.agent ? <SquareTerminal size={12} className="text-white/50" /> : <Layers size={12} className="text-[#d9a441]" />}
            {t.label}
          </span>
        ))}
      </div>
      <div className="min-h-0 flex-1">
        {view === "board" ? <Board stage={stage} answered={answered} /> : <Terminal key={view} task={view} />}
      </div>
    </div>
  );
}

const TERMINAL: Record<"t1" | "t2", { agent: Agent; name: string; model: string; lines: { kind: "do" | "run" | "ok" | "say"; text: string }[] }> = {
  t1: {
    agent: "codex",
    name: "Codex",
    model: "gpt-6",
    lines: [
      { kind: "do", text: "Read src/payments/client.ts, src/payments/types.ts" },
      { kind: "do", text: "Searched 'amountCents' · 14 uses in 6 files" },
      { kind: "do", text: "Edited src/payments/client.ts  +84 −12" },
      { kind: "do", text: "Edited src/checkout/submit.ts  +22 −18" },
      { kind: "do", text: "Edited src/payments/types.ts  +9 −3" },
      { kind: "run", text: "pnpm test payments" },
      { kind: "ok", text: "38 passed (1.9s)" },
      { kind: "say", text: "Told t2: Amount is now Decimal, not cents." },
    ],
  },
  t2: {
    agent: "opencode",
    name: "OpenCode",
    model: "big-pickle",
    lines: [
      { kind: "do", text: "Read src/api/legacy/checkout.ts, src/api/routes.ts" },
      { kind: "do", text: "Found 3 endpoints old mobile clients still call" },
      { kind: "do", text: "Added toDecimal() at the edge  +41" },
      { kind: "do", text: "Edited src/api/legacy/checkout.ts  +12 −4" },
      { kind: "run", text: "pnpm test legacy" },
      { kind: "ok", text: "22 passed (1.1s)" },
      { kind: "say", text: "Asked the lead: convert old clients' cents, or version it?" },
    ],
  },
};

/** A worker's own terminal: its agent at work, line by line. */
function Terminal({ task }: { task: "t1" | "t2" }) {
  const run = TERMINAL[task];
  const [shown, setShown] = useState(1);
  useEffect(() => {
    const tick = setInterval(() => setShown((n) => Math.min(run.lines.length, n + 1)), 380);
    return () => clearInterval(tick);
  }, [run.lines.length]);
  return (
    <div className="h-full bg-[#0c1015] px-5 py-4 font-[family-name:var(--font-mono-jb)] text-[12px] leading-[1.75]">
      <p className="flex items-center gap-2 text-white/80">
        <AgentFace agent={run.agent} size={18} />
        <span className="font-semibold">{run.name}</span>
        <span className="text-white/35">· {run.model} · {task}-billing</span>
      </p>
      <p className="mt-3 text-white/55">
        <span className="text-[#8fd18a]">›</span> {TASKS.find((t) => t.id === task)?.title}: follow the plan in .dragent/team
      </p>
      {run.lines.slice(0, shown).map((line) => (
        <p key={line.text} className="text-white/70">
          {line.kind === "run" ? (
            <><span className="text-[#75baf2]">$</span> {line.text}</>
          ) : line.kind === "ok" ? (
            <span className="pl-3 text-[#34d399]">✓ {line.text}</span>
          ) : line.kind === "say" ? (
            <span className="text-[#f6a96e]">✉ {line.text}</span>
          ) : (
            <><span className="text-white/35">•</span> {line.text}</>
          )}
        </p>
      ))}
      <span className="mt-1 inline-block h-[14px] w-[7px] translate-y-[2px] animate-pulse bg-white/70" />
    </div>
  );
}

/** The pointer: glides to what it's about to click; a ring marks each click. */
function Pointer({ x, y, shown, press }: { x: number; y: number; shown: boolean; press: number }) {
  return (
    <motion.div
      aria-hidden
      className="pointer-events-none absolute left-0 top-0 z-30"
      initial={false}
      animate={{ x, y, opacity: shown ? 1 : 0 }}
      transition={{ x: { duration: 0.8, ease: [0.45, 0, 0.2, 1] }, y: { duration: 0.8, ease: [0.45, 0, 0.2, 1] }, opacity: { duration: 0.25 } }}
    >
      {press > 0 && <span key={press} className="demo-click absolute -left-[14px] -top-[14px] h-7 w-7 rounded-full border-2 border-white/80" />}
      <motion.svg
        key={`p${press}`}
        width="20"
        height="22"
        viewBox="0 0 20 22"
        className="relative -translate-x-[3px] -translate-y-[2px] drop-shadow-[0_2px_4px_rgba(0,0,0,0.6)]"
        initial={{ scale: press > 0 ? 0.82 : 1 }}
        animate={{ scale: 1 }}
        transition={{ duration: 0.25 }}
      >
        <path d="M3 2 L3 18 L7.2 14.1 L10 20.2 L12.9 18.9 L10.2 12.9 L16 12.9 Z" fill="#fff" stroke="#0b0d10" strokeWidth="1.4" strokeLinejoin="round" />
      </motion.svg>
    </motion.div>
  );
}

function Board({ stage, answered }: { stage: Stage; answered: boolean }) {
  const planning = stage === "planning";
  const planned = stageIndex(stage) >= stageIndex("planned");
  const needs = stage === "needs";
  const ready = stage === "ready";
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-1 border-b border-white/[0.07] px-5 pt-3">
        {["Overview", "Board", "Activity"].map((t) => (
          <span key={t} className={`px-3 pb-2.5 text-[12.5px] ${t === "Board" ? "border-b-2 border-[#75baf2] text-white/90" : "text-white/45"}`}>{t}</span>
        ))}
        <span className="ml-auto pb-2.5 text-[11.5px] text-white/35">Open lead ›</span>
      </div>
      <div className="px-5 pb-3 pt-4">
        <span className={`inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] ${ready ? "bg-[#34d399]/12 text-[#34d399]" : needs ? "bg-[#f08a3c]/14 text-[#f6a96e]" : "bg-[#75baf2]/12 text-[#75baf2]"}`}>
          {ready ? <Check size={11} /> : needs ? <TriangleAlert size={11} /> : <Clock size={11} />}
          {ready ? "Ready for review" : needs ? "Needs you" : planning ? "Planning" : "At work"}
        </span>
        <p className="mt-2 text-[17px] font-semibold tracking-[-0.01em] text-white/90">Billing API migration</p>
        <p className="mt-1 flex items-center gap-1.5 text-[11.5px] text-white/45">
          <AgentFace agent="claude" size={15} /> Led by Claude Code · Opus · 3 tasks
        </p>
      </div>

      {needs && (
        <div
          className="mx-5 mb-3 flex flex-wrap items-center gap-2.5 rounded-xl border border-[#f08a3c]/40 bg-[#f08a3c]/[0.08] px-3.5 py-2.5 text-[12.5px] text-white/70"
        >
          <MessageSquare size={14} className="text-[#f6a96e]" />
          {answered ? (
            <span className="min-w-0 flex-1"><b className="font-medium text-white/90">You answered:</b> Convert at the edge. The lead passes it to t2.</span>
          ) : (
            <>
              <span className="min-w-0 flex-1"><b className="font-medium text-white/90">The lead asks:</b> Old clients send cents, the new API wants decimals. Convert at the edge, or version the endpoint?</span>
              <span data-cursor="answer" className="rounded-md bg-[#f08a3c] px-2.5 py-1 text-[12px] font-semibold text-[#1d0d02]">
                Convert at the edge
              </span>
              <span className="rounded-md bg-white/[0.08] px-2.5 py-1 text-[12px] text-white/80">Version it</span>
            </>
          )}
        </div>
      )}
      {ready && (
        <div
          className="mx-5 mb-3 flex flex-wrap items-center gap-3 rounded-xl border border-[#34d399]/30 bg-[#34d399]/[0.07] px-3.5 py-2.5 text-[12.5px] text-white/70"
        >
          <Check size={14} className="text-[#34d399]" />
          <span className="flex-1"><b className="font-medium text-white/90">Ready for review:</b> 3 tasks · 412 tests pass · <span className="text-[#34d399]">+508</span> <span className="text-[#f4677a]">−42</span></span>
          <span data-cursor="review" className="rounded-md bg-white/90 px-2.5 py-1 text-[12px] font-semibold text-[#0c1015]">Review changes</span>
        </div>
      )}

      <LayoutGroup>
        <div className="grid min-h-0 flex-1 grid-cols-3 gap-2 overflow-hidden px-5 pb-5 lg:grid-cols-5">
          {COLUMNS.map((col) => {
            const tasks = planned ? TASKS.filter((t) => columnOf(t.id, stage) === col.id) : [];
            const lead = needs && col.id === "needs";
            const count = tasks.length + (lead ? 1 : 0);
            return (
              <div key={col.id} className={`flex min-w-0 flex-col rounded-[10px] bg-white/[0.03] p-2 ${col.id === "review" || col.id === "needs" ? "hidden lg:flex" : ""}`}>
                <span className="mb-2 flex items-center gap-1.5 px-1 text-[12px] font-medium text-white/85">
                  <span className="h-1.5 w-1.5 rounded-full" style={{ background: col.color }} />
                  {col.label}
                  <span className="text-white/35">{count}</span>
                </span>
                {col.id === "next" && planning && <PlanningCard />}
                {lead && (
                  <motion.div layoutId="lead-ask" className="mb-2 rounded-lg border border-[#f08a3c]/50 bg-[#0c1015] p-2.5">
                    <span className="flex items-center justify-between font-[family-name:var(--font-mono-jb)] text-[10px] text-white/40">Lead <AgentFace agent="claude" size={16} /></span>
                    <p className="mt-1.5 text-[12px] text-white/85">Cents or decimals for old clients?</p>
                    <span className="mt-2 inline-block rounded border border-[#f08a3c]/50 px-1.5 font-[family-name:var(--font-mono-jb)] text-[10px] text-[#f6a96e]">needs you</span>
                  </motion.div>
                )}
                {tasks.map((t) => (
                  <TaskCard key={t.id} task={t} column={col.id} stage={stage} />
                ))}
                {count === 0 && !(col.id === "next" && planning) && (
                  <span className="px-1 text-[11.5px] text-white/30">{col.id === "next" ? "Nothing waiting." : col.id === "done" ? "Nothing finished yet." : "—"}</span>
                )}
              </div>
            );
          })}
        </div>
      </LayoutGroup>
    </div>
  );
}

function PlanningCard() {
  return (
    <div className="edge-lap relative mb-2 rounded-lg border border-white/10 bg-[#0c1015] p-2.5">
      <span className="flex items-center gap-2 text-[12px] font-medium text-white/85">
        <AgentFace agent="claude" size={20} pulse={0} />
        Claude Code is writing the plan
      </span>
      <span className="mt-2.5 block h-1.5 w-[75%] rounded bg-white/[0.08]" />
      <span className="mt-2 block h-1.5 w-[52%] rounded bg-white/[0.08]" />
      <span className="mt-2.5 block text-[10.5px] text-white/35">Tasks land here as it decides them.</span>
    </div>
  );
}

function TaskCard({ task, column, stage }: { task: (typeof TASKS)[number]; column: Column; stage: Stage }) {
  const working = column === "working";
  const done = column === "done";
  const waiting = column === "next" && task.id === "t3";
  const note =
    working && task.id === "t1" && stage === "working"
      ? { from: "t1 → t2", text: "Amount is now Decimal, not cents. Updated the shared type." }
      : working && task.id === "t2" && stage === "working"
        ? { from: "t2 → t1", text: "Got it, converting at the edge." }
        : working && task.qa
          ? { from: "QA", text: "Running 412 tests and the refund cases…" }
          : null;
  return (
    <motion.div
      layoutId={task.id}
      layout
      transition={{ type: "spring", stiffness: 260, damping: 30 }}
      className={`relative mb-2 rounded-lg border bg-[#0c1015] p-2.5 ${working ? "edge-lap-amber border-[#d9a441]/25" : "border-white/[0.08]"}`}
    >
      <span className="flex items-center justify-between font-[family-name:var(--font-mono-jb)] text-[10px] text-white/40">
        <span>{task.id}{task.qa ? " · QA" : ""}</span>
        <span className="flex items-center gap-1.5">
          {(working || done) && (
            <span data-cursor={`term-${task.id}`} title="Open its terminal" className="flex h-5 w-5 items-center justify-center rounded text-white/50 hover:bg-white/10">
              <SquareTerminal size={12} />
            </span>
          )}
          <AgentFace agent={task.agent} size={16} />
        </span>
      </span>
      <p className="mt-1.5 text-[12px] leading-snug text-white/85">{task.title}</p>
      <span className="mt-2 flex flex-wrap gap-1 font-[family-name:var(--font-mono-jb)] text-[10px]">
        <span className={`rounded border px-1.5 ${working ? "border-[#d9a441]/40 text-[#d9a441]" : done ? "border-[#34d399]/40 text-[#34d399]" : "border-white/10 text-white/45"}`}>
          {working ? "working" : done ? (task.qa ? "passed" : "approved") : waiting ? "waits for t1" : "ready"}
        </span>
        <span className="rounded border border-white/10 px-1.5 text-white/45">{task.model}</span>
      </span>
      {note && (
        <p className="mt-2 rounded-md bg-white/[0.04] px-2 py-1.5 text-[11px] leading-snug text-white/55">
          <b className="font-medium text-white/80">{note.from}:</b> {note.text}
        </p>
      )}
    </motion.div>
  );
}
