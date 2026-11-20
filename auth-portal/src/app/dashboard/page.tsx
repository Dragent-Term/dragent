"use client";

import React, { useEffect, useMemo, useState } from "react";
import {
  Loader2,
  Sparkles,
  CreditCard,
  Wallet,
  Check,
  ExternalLink,
  LayoutDashboard,
  Gauge,
  Coins,
  Settings as SettingsIcon,
  LogOut,
  Cpu,
  Clock,
  Zap,
  ArrowUpRight,
  ArrowLeft,
  Building2,
} from "lucide-react";
import SessionControl from "@/components/SessionControl";
import { getToken, setToken as persistToken, clearToken, adoptLegacySessionToken } from "@/lib/token";
import { readHandoff, clearHandoff, type DesktopHandoff } from "@/lib/desktopHandoff";
import OrgSwitcher, { type OrgWithRole } from "@/components/organization/OrgSwitcher";
import OrganizationSection, { MyInvitationsBanner } from "@/components/organization/OrganizationSection";

// ── Types mirroring the Go API (/api/v1/account, /billing/plans, /packs) ──────

interface Account {
  user: {
    email: string;
    display_name: string;
    first_name: string;
    last_name: string;
    avatar_url: string;
    tier: string;
    created_at: number;
  };
  usage: {
    paid: {
      is_paid: boolean;
      cap_mc: number;
      used_mc: number;
      balance_mc: number;
      resets_at: number;
    };
    free: {
      cap_mc: number;
      used_mc: number;
      resets_at: number;
    };
    deposit_balance_mc: number;
    allow_credit_usage: boolean;
  };
  models: {
    id: string;
    name: string;
    provider: string;
    context_window: number;
    min_tier: string;
    is_active: boolean;
  }[];
}
interface Plan {
  id: string;
  tier: string;
  billing_interval: "monthly" | "annual";
  price_cents: number;
  currency: string;
  display_name: string;
  purchasable: boolean;
}
interface Pack {
  id: string;
  price_cents: number;
  credits_mc: number;
  currency: string;
  display_name: string;
  purchasable: boolean;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

const usd = (dollars: number) =>
  dollars.toLocaleString("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 2 });
const fromCents = (c: number) => usd(c / 100);
const pct = (used: number, cap: number) => (cap > 0 ? Math.min(100, Math.round((used / cap) * 100)) : 0);
// Purchased "credits" are shown as a count, not a dollar value: 1 credit = 1¢
// (so $1 = 100 credits; a $50 pack with +15% = 5,750 credits). 1 credit = 10,000 mc.
// Context window: millions as "M" (1,000,000 → "1M", 1,048,576 → "1.05M"),
// thousands as "K". Dividing by 1000 alone rendered a 1M window as "1,000K".
function fmtCtx(n: number): string {
  if (n >= 1_000_000) {
    const m = (n / 1_000_000).toFixed(2).replace(/\.?0+$/, "");
    return `${m}M ctx`;
  }
  if (n >= 1_000) return `${Math.round(n / 1000).toLocaleString()}K ctx`;
  return `${n} ctx`;
}
const toCredits = (mc: number) => Math.round(mc / 10_000).toLocaleString("en-US");
const fmtDate = (unixSec: number) =>
  unixSec ? new Date(unixSec * 1000).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" }) : "—";
const initials = (name: string, email: string) => {
  const n = (name || email || "?").trim();
  const parts = n.split(/\s+/);
  return (parts.length > 1 ? parts[0][0] + parts[1][0] : n.slice(0, 2)).toUpperCase();
};

// `undefined` = still resolving; `null` = no session; string = signed in.
function useToken(): [string | null | undefined, (t: string) => void] {
  const [token, setToken] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    const hash = window.location.hash.replace(/^#/, "");
    const fromHash = new URLSearchParams(hash).get("token");
    if (fromHash) {
      persistToken(fromHash);
      history.replaceState(null, "", window.location.pathname);
      setToken(fromHash);
      return;
    }
    // Anyone mid-session when this shipped still has a token in sessionStorage;
    // promote it rather than signing them out on the upgrade itself.
    setToken(adoptLegacySessionToken() ?? getToken());
  }, []);
  return [token, setToken];
}

type Section = "overview" | "usage" | "billing" | "credits" | "team" | "settings";
const NAV: { key: Section; label: string; icon: typeof LayoutDashboard }[] = [
  { key: "overview", label: "Overview", icon: LayoutDashboard },
  { key: "usage", label: "Usage", icon: Gauge },
  { key: "billing", label: "Plans & billing", icon: CreditCard },
  { key: "credits", label: "Credits", icon: Coins },
  { key: "team", label: "Team", icon: Building2 },
  { key: "settings", label: "Settings", icon: SettingsIcon },
];

// ── Page ──────────────────────────────────────────────────────────────────────

export default function DashboardPage() {
  const [token, setToken] = useToken();
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [account, setAccount] = useState<Account | null>(null);
  const [plans, setPlans] = useState<Plan[]>([]);
  const [packs, setPacks] = useState<Pack[]>([]);
  const [section, setSection] = useState<Section>("overview");
  const [billingInterval, setBillingInterval] = useState<"monthly" | "annual">("annual");
  const [busy, setBusy] = useState<string | null>(null);

  // A pending "Open in Dragent" hand-off, left by /login when the desktop app
  // started this sign-in. Read once on mount; dismissing clears it so a later
  // dashboard visit in the same tab does not re-prompt.
  //
  // MUST live with the other hooks, above every early return: this component
  // returns early for both "still loading" and "no session", so a hook declared
  // below them runs on some renders and not others — which is what crashed the
  // page with "rendered more hooks than during the previous render".
  // One login flow: no session means /login, which handles both sign-in and
  // registration. Redirecting from an effect rather than during render —
  // navigating mid-render is a side effect React does not allow.
  useEffect(() => {
    if (token === null) window.location.replace("/login");
  }, [token]);

  // Links from Organization retain the dashboard section the person chose.
  // Read this after hydration so server and client render the same initial UI.
  useEffect(() => {
    const requested = new URLSearchParams(window.location.search).get("section");
    if (requested && NAV.some((item) => item.key === requested)) setSection(requested as Section);
  }, []);

  const [handoff, setHandoff] = useState<DesktopHandoff | null>(null);
  useEffect(() => {
    setHandoff(readHandoff());
  }, []);
  const dismissHandoff = () => {
    clearHandoff();
    setHandoff(null);
  };

  // Every account has at least one organization (the personal one
  // auto-created at signup) — this resolves it once the token is ready, and
  // OrgSwitcher/MyInvitationsBanner/OrganizationSection all share this one
  // copy rather than each fetching it independently.
  const [activeOrg, setActiveOrg] = useState<OrgWithRole | null>(null);
  useEffect(() => {
    if (!token) return;
    fetch("/api/organization/me/active-organization", { headers: { Authorization: `Bearer ${token}` } })
      .then((res) => (res.ok ? res.json() : null))
      .then((org) => org && setActiveOrg(org))
      .catch(() => {});
  }, [token]);

  const api = useMemo(
    () =>
      async (path: string, init?: RequestInit) => {
        const res = await fetch(path, {
          ...init,
          headers: {
            ...(init?.headers ?? {}),
            Authorization: `Bearer ${token ?? ""}`,
            "Content-Type": "application/json",
          },
        });
        const data = await res.json().catch(() => ({}));

        // A rejected token is a dead end unless we clear it. Tokens now live in
        // localStorage, so a stale one survives tab closes — without this the
        // dashboard renders the backend's raw "authentication required: provide
        // a valid Bearer token" on every load, forever, with no way out.
        // Treat 401 as "this session is over": drop it and start sign-in again.
        // Only /api/account is authoritative about whether the session is
        // still good. Signing out on a 401 from any endpoint means one
        // misconfigured route logs everyone out of a perfectly valid session —
        // and the redirect hides which call actually failed.
        if (res.status === 401) {
          if (path.startsWith("/api/account")) {
            clearToken();
            clearHandoff();
            window.location.replace("/login");
            throw new Error("session expired");
          }
          throw new Error(
            `${path} rejected the session (401). ${data?.error ?? ""}`.trim()
          );
        }

        if (!res.ok) throw new Error(data?.error ?? `request failed (${res.status})`);
        return data;
      },
    [token]
  );

  useEffect(() => {
    if (token === undefined) return;
    if (!token) {
      setReady(true);
      return;
    }
    setReady(false);
    (async () => {
      try {
        const [acc, pl, pk] = await Promise.all([
          api("/api/account"),
          api("/api/billing/plans"),
          api("/api/billing/packs"),
        ]);
        setAccount(acc);
        setPlans(pl.plans ?? []);
        setPacks(pk.packs ?? []);
      } catch (e) {
        setError(e instanceof Error ? e.message : "failed to load account");
      } finally {
        setReady(true);
      }
    })();
  }, [token, api]);

  const go = async (path: string, body: object, key: string) => {
    setBusy(key);
    try {
      const { url } = await api(path, { method: "POST", body: JSON.stringify(body) });
      window.location.href = url;
    } catch (e) {
      setError(e instanceof Error ? e.message : "checkout failed");
      setBusy(null);
    }
  };
  const manage = async () => {
    setBusy("portal");
    try {
      const { url } = await api("/api/billing/portal");
      window.location.href = url;
    } catch (e) {
      setError(e instanceof Error ? e.message : "could not open billing portal");
      setBusy(null);
    }
  };
  const signOut = async () => {
    // End the session on the server too, so this token stops working even if
    // it was copied. Sign out locally whatever the answer.
    try {
      await fetch("/api/auth/logout", {
        method: "POST",
        headers: { Authorization: `Bearer ${token ?? ""}` },
      });
    } catch {
      /* offline: the local sign-out still happens */
    }
    clearToken();
    clearHandoff();
    window.location.reload();
  };

  if (!ready) {
    return (
      <Center>
        <Loader2 className="w-6 h-6 animate-spin text-[#19aad8]" />
      </Center>
    );
  }
  // One login flow. The dashboard used to render its own <SignIn/>, which
  // hardcoded is_registration:false — so a visitor without an account could
  // never create one here, signed in against nothing, and saw an empty
  // dashboard. Redirecting to /login means there is a single implementation
  // that handles both sign-in and registration.
  if (!token) return null;

  const u = account?.user;
  const tier = u?.tier ?? "free";
  const name = u?.display_name || u?.first_name || (u?.email ? u.email.split("@")[0] : "there");

  return (
    <div className="flex min-h-screen w-full">
      {handoff && (
        <div className="fixed inset-x-0 top-0 z-50 flex justify-center px-4 pt-4">
          <div className="w-full max-w-md rounded-2xl border border-white/10 bg-[#0b0b0e] p-4 shadow-2xl">
            <div className="mb-3 flex items-start justify-between gap-3">
              <p className="text-sm text-white/70">
                You signed in from the Dragent app. Open it to finish linking.
              </p>
              <button
                onClick={dismissHandoff}
                className="shrink-0 text-xs text-white/40 transition-colors hover:text-white/80"
              >
                Dismiss
              </button>
            </div>
            <SessionControl
              session={{ code: handoff.code, refresh_token: handoff.refresh_token, user_id: handoff.user_id }}
              scheme={handoff.scheme}
              state={handoff.state}
            />
          </div>
        </div>
      )}
      {/* Sidebar */}
      <aside className="sticky top-0 hidden h-screen w-60 shrink-0 self-start flex-col overflow-y-auto border-r border-white/8 bg-[#070709] px-3 py-5 md:flex">
        <div className="flex items-center gap-2.5 px-2 mb-5">
          <img src="/dragent-icon.png" alt="" className="w-7 h-7 rounded-[22%]" />
          <span className="font-[family-name:var(--font-outfit)] font-semibold tracking-tight text-[1.05rem]">
            Dragent
          </span>
        </div>

        {/* Org switcher lives here, at the top of the persistent sidebar —
            reachable from every section, not buried on a page you have to
            already be on to use. */}
        {token && <OrgSwitcher token={token} active={activeOrg} onActiveOrgChanged={setActiveOrg} />}

        <nav className="flex flex-col gap-1">
          {NAV.map((n) => (
            <button
              key={n.key}
              onClick={() => setSection(n.key)}
              className={`flex items-center gap-3 rounded-lg px-3 py-2.5 text-sm transition-colors ${
                section === n.key
                  ? "bg-[#19aad8]/12 text-white border border-[#19aad8]/25"
                  : "text-white/55 hover:text-white hover:bg-white/[0.04] border border-transparent"
              }`}
            >
              <n.icon className={`w-4 h-4 ${section === n.key ? "text-[#4fd4f6]" : ""}`} />
              {n.label}
            </button>
          ))}
        </nav>

        <div className="mt-auto">
          <div className="flex items-center gap-2.5 rounded-xl border border-white/8 bg-white/[0.02] p-3">
            <Avatar u={u} />
            <div className="min-w-0 flex-1">
              <div className="truncate text-[13px] font-medium">{name}</div>
              <div className="truncate text-[11px] text-white/40">{u?.email}</div>
            </div>
          </div>
          <button
            onClick={signOut}
            className="mt-2 flex w-full items-center gap-2 rounded-lg px-3 py-2 text-[13px] text-white/50 hover:text-white hover:bg-white/[0.04] transition-colors"
          >
            <LogOut className="w-4 h-4" /> Sign out
          </button>
        </div>
      </aside>

      {/* Main */}
      <main className="min-h-screen min-w-0 flex-1">
        {/* Top bar */}
        <div className="sticky top-0 z-20 flex items-center gap-3 border-b border-white/8 bg-[#030303]/80 backdrop-blur-xl px-5 sm:px-8 h-16">
          <h1 className="font-[family-name:var(--font-outfit)] text-lg font-semibold capitalize">
            {NAV.find((n) => n.key === section)?.label}
          </h1>
          <div className="ml-auto flex items-center gap-3">
            <TierBadge tier={tier} />
            {account && (
              <span className="hidden sm:inline-flex items-center gap-1.5 rounded-full border border-white/10 px-3 py-1 text-xs text-white/60">
                <Wallet className="w-3.5 h-3.5 text-[#4fd4f6]" />
                {toCredits(account.usage.deposit_balance_mc)} credits
              </span>
            )}
          </div>
        </div>

        {/* Mobile nav */}
        <div className="md:hidden flex gap-1 overflow-x-auto border-b border-white/8 px-4 py-2">
          {NAV.map((n) => (
            <button
              key={n.key}
              onClick={() => setSection(n.key)}
              className={`shrink-0 rounded-lg px-3 py-1.5 text-[13px] ${
                section === n.key ? "bg-[#19aad8]/15 text-white" : "text-white/50"
              }`}
            >
              {n.label}
            </button>
          ))}
        </div>

        <div className="px-5 sm:px-8 py-8 max-w-5xl mx-auto">
          {error && (
            <div className="mb-6 rounded-xl border border-[#f43f5e]/30 bg-[#f43f5e]/10 px-4 py-3 text-sm text-[#fca5b4]">
              {error}
            </div>
          )}

          {/* Shown on every section, not just Team — an invitation is
              usually for a DIFFERENT org than whatever you're currently
              looking at, so it wouldn't otherwise be reachable from here. */}
          {token && <div className="mb-6"><MyInvitationsBanner token={token} onAccepted={() => window.location.reload()} /></div>}

          {account && section === "overview" && (
            <Overview account={account} name={name} onGo={setSection} onManage={manage} busy={busy} />
          )}
          {account && section === "usage" && <UsageSection account={account} />}
          {section === "billing" && (
            <BillingSection
              tier={tier}
              plans={plans}
              interval={billingInterval}
              setInterval={setBillingInterval}
              onSubscribe={(id) => go("/api/billing/checkout", { plan_id: id }, id)}
              onManage={manage}
              busy={busy}
            />
          )}
          {section === "credits" && (
            <CreditsSection
              packs={packs}
              depositBalance={account?.usage.deposit_balance_mc ?? 0}
              onBuy={(id) => go("/api/billing/topup", { pack_id: id }, id)}
              busy={busy}
            />
          )}
          {section === "team" && (
            token && activeOrg ? (
              <OrganizationSection token={token} org={activeOrg} />
            ) : (
              <div className="flex justify-center py-10"><Loader2 className="w-5 h-5 animate-spin text-white/30" /></div>
            )
          )}
          {account && section === "settings" && <SettingsSection account={account} onSignOut={signOut} />}
        </div>
      </main>
    </div>
  );
}

// ── Sections ──────────────────────────────────────────────────────────────────

function Overview({
  account,
  name,
  onGo,
  onManage,
  busy,
}: {
  account: Account;
  name: string;
  onGo: (s: Section) => void;
  onManage: () => void;
  busy: string | null;
}) {
  const paid = account.usage.paid;
  const free = account.usage.free;
  const deposit = account.usage.deposit_balance_mc;
  const tier = account.user.tier;
  // One allowance per user: paid users see their monthly allowance, free users
  // their free monthly allowance. Both renew monthly; no weekly/session throttle.
  const isPaid = tier !== "free";
  const allowUsed = isPaid ? paid.used_mc : free.used_mc;
  const allowCap = isPaid ? paid.cap_mc : free.cap_mc;
  const usePct = pct(allowUsed, allowCap);
  return (
    <div className="space-y-8">
      <div>
        <h2 className="font-[family-name:var(--font-outfit)] text-2xl font-semibold tracking-tight">
          Welcome back, {name.split(" ")[0]}.
        </h2>
        <p className="text-sm text-white/45 mt-1">Here&apos;s your account at a glance.</p>
      </div>

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard icon={<Sparkles className="w-4 h-4" />} label="Plan" value={<span className="capitalize">{tier}</span>} hint={tier === "free" ? "Upgrade for more" : "Active"} />
        <StatCard icon={<Gauge className="w-4 h-4" />} label="Monthly usage" value={`${usePct}%`} hint="of your monthly allowance" accent />
        <StatCard icon={<Coins className="w-4 h-4" />} label="Top-up credits" value={toCredits(deposit)} hint="1 credit = 1¢ · never expire" />
      </div>

      <Card>
        <h3 className="font-[family-name:var(--font-outfit)] font-semibold mb-4">Monthly usage</h3>
        {/* Paid users have a plan allowance (spent first) AND a free allowance;
            free users just the free one. Show each meter the user has. */}
        {isPaid && (
          <>
            <Meter label="Plan allowance" used={paid.used_mc} cap={paid.cap_mc} />
            <p className="text-xs text-white/40 mt-2 mb-4 flex items-center gap-1.5">
              <Clock className="w-3.5 h-3.5" /> Resets {fmtDate(paid.resets_at)}
            </p>
          </>
        )}
        <Meter label="Free allowance" used={free.used_mc} cap={free.cap_mc} />
        <p className="text-xs text-white/40 mt-2 flex items-center gap-1.5">
          <Clock className="w-3.5 h-3.5" /> Resets {fmtDate(free.resets_at)}
        </p>
      </Card>

      <div className="grid sm:grid-cols-3 gap-4">
        <ActionCard icon={<ArrowUpRight className="w-4 h-4" />} title={tier === "free" ? "Upgrade plan" : "Change plan"} desc="More monthly usage" onClick={() => onGo("billing")} />
        <ActionCard icon={<Coins className="w-4 h-4" />} title="Buy credits" desc="Top up any time" onClick={() => onGo("credits")} />
        {tier !== "free" ? (
          <ActionCard icon={busy === "portal" ? <Loader2 className="w-4 h-4 animate-spin" /> : <CreditCard className="w-4 h-4" />} title="Manage billing" desc="Invoices & payment" onClick={onManage} />
        ) : (
          <ActionCard icon={<Cpu className="w-4 h-4" />} title="View usage" desc="Detailed breakdown" onClick={() => onGo("usage")} />
        )}
      </div>
    </div>
  );
}

function UsageSection({ account }: { account: Account }) {
  const { paid, free } = account.usage;
  const isPaid = account.user.tier !== "free";
  const activeModels = account.models.filter((m) => m.is_active);
  return (
    <div className="space-y-6">
      <Card>
        <h3 className="font-[family-name:var(--font-outfit)] font-semibold mb-4">This month</h3>
        {/* Every user has a free monthly allowance; paid users also have a
            plan allowance that is spent FIRST, then the free one, then credits.
            Show each meter the user actually has. */}
        {isPaid && (
          <>
            <Meter label="Plan allowance" used={paid.used_mc} cap={paid.cap_mc} />
            <p className="text-xs text-white/40 mt-2 mb-4 flex items-center gap-1.5">
              <Clock className="w-3.5 h-3.5" /> Resets {fmtDate(paid.resets_at)}
            </p>
          </>
        )}
        <Meter label="Free allowance" used={free.used_mc} cap={free.cap_mc} />
        <p className="text-xs text-white/40 mt-2 flex items-center gap-1.5">
          <Clock className="w-3.5 h-3.5" /> Resets {fmtDate(free.resets_at)}
        </p>
      </Card>

      <Card>
        <div className="flex items-center gap-2 mb-4">
          <Cpu className="w-4 h-4 text-[#4fd4f6]" />
          <h3 className="font-[family-name:var(--font-outfit)] font-semibold">Available models</h3>
          <span className="text-xs text-white/40">({activeModels.length})</span>
        </div>
        <div className="divide-y divide-white/6">
          {activeModels.map((m) => (
            <div key={m.id} className="flex items-center gap-3 py-2.5">
              <div className="min-w-0 flex-1">
                <div className="text-sm truncate">{m.name}</div>
                <div className="text-[11px] text-white/40 capitalize">{m.provider}</div>
              </div>
              <span className="text-[11px] text-white/45 tabular-nums">
                {fmtCtx(m.context_window)}
              </span>
              {m.min_tier !== "free" && (
                <span className="text-[10px] rounded-full bg-[#19aad8]/15 text-[#9be7ff] px-2 py-0.5 capitalize">
                  {m.min_tier}
                </span>
              )}
            </div>
          ))}
          {activeModels.length === 0 && <p className="text-sm text-white/40 py-2">No models available.</p>}
        </div>
      </Card>
    </div>
  );
}

function BillingSection({
  tier,
  plans,
  interval,
  setInterval,
  onSubscribe,
  onManage,
  busy,
}: {
  tier: string;
  plans: Plan[];
  interval: "monthly" | "annual";
  setInterval: (v: "monthly" | "annual") => void;
  onSubscribe: (id: string) => void;
  onManage: () => void;
  busy: string | null;
}) {
  const monthlyList = plans.filter((p) => p.billing_interval === "monthly");
  const shown = plans.filter((p) => p.billing_interval === interval);
  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between flex-wrap gap-3">
        <div>
          <h2 className="font-[family-name:var(--font-outfit)] text-xl font-semibold">
            {tier === "free" ? "Choose a plan" : "Change plan"}
          </h2>
          <p className="text-sm text-white/45 mt-1">All plans unlock every model — they differ by monthly usage.</p>
        </div>
        <div className="flex items-center gap-3">
          {tier !== "free" && (
            <button
              onClick={onManage}
              disabled={busy === "portal"}
              className="inline-flex items-center gap-2 rounded-lg border border-white/10 px-4 py-2 text-sm text-white/70 hover:bg-white/5 transition disabled:opacity-50"
            >
              {busy === "portal" ? <Loader2 className="w-4 h-4 animate-spin" /> : <CreditCard className="w-4 h-4" />}
              Manage billing
            </button>
          )}
          <IntervalToggle value={interval} onChange={setInterval} />
        </div>
      </div>

      <div className="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
        {shown.map((p) => {
          const monthly = monthlyList.find((m) => m.tier === p.tier);
          const perMonth = p.billing_interval === "annual" ? p.price_cents / 12 / 100 : p.price_cents / 100;
          const savePct =
            p.billing_interval === "annual" && monthly
              ? Math.round((1 - p.price_cents / (monthly.price_cents * 12)) * 100)
              : 0;
          const current = p.tier === tier;
          const featured = p.tier === "pro";
          return (
            <div
              key={p.id}
              className={`relative rounded-2xl p-5 flex flex-col ${
                featured ? "ribbon-border" : "border border-white/10 bg-white/[0.02]"
              }`}
            >
              {featured && (
                <span className="absolute -top-3 left-5 rounded-full bg-gradient-to-r from-[#2fd4c8] to-[#19aad8] px-2.5 py-0.5 text-[11px] font-semibold text-[#05070a]">
                  Popular
                </span>
              )}
              <div className="flex items-baseline justify-between">
                <h3 className="font-medium capitalize">{p.tier}</h3>
                {savePct > 0 && (
                  <span className="text-[11px] rounded-full bg-[#19aad8]/15 text-[#9be7ff] px-2 py-0.5">Save {savePct}%</span>
                )}
              </div>
              <div className="mt-3 mb-1">
                <span className="text-3xl font-semibold tabular-nums">{usd(perMonth)}</span>
                <span className="text-white/40 text-sm"> / mo</span>
              </div>
              <p className="text-xs text-white/40 mb-5">
                {p.billing_interval === "annual" ? `Billed ${fromCents(p.price_cents)}/yr` : "Billed monthly"}
              </p>
              {current ? (
                <div className="mt-auto inline-flex items-center justify-center gap-2 rounded-lg border border-white/10 px-4 py-2 text-sm text-white/50">
                  <Check className="w-4 h-4" /> Current plan
                </div>
              ) : (
                <button
                  onClick={() => onSubscribe(p.id)}
                  disabled={!p.purchasable || busy === p.id}
                  className={`mt-auto inline-flex items-center justify-center gap-2 rounded-lg px-4 py-2 text-sm font-medium transition disabled:opacity-40 ${
                    featured
                      ? "bg-gradient-to-r from-[#19aad8] to-[#3f7fe0] text-white"
                      : "bg-white/10 hover:bg-white/15 text-white"
                  }`}
                  title={p.purchasable ? "" : "Not available yet"}
                >
                  {busy === p.id ? <Loader2 className="w-4 h-4 animate-spin" /> : <ExternalLink className="w-4 h-4" />}
                  {tier === "free" ? "Subscribe" : "Switch"}
                </button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function CreditsSection({
  packs,
  depositBalance,
  onBuy,
  busy,
}: {
  packs: Pack[];
  depositBalance: number;
  onBuy: (id: string) => void;
  busy: string | null;
}) {
  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between flex-wrap gap-3">
        <div>
          <h2 className="font-[family-name:var(--font-outfit)] text-xl font-semibold">Buy credits</h2>
          <p className="text-sm text-white/45 mt-1">One-time top-ups that never expire and stack on your plan.</p>
        </div>
        <span className="inline-flex items-center gap-2 rounded-full border border-white/10 px-3 py-1.5 text-sm text-white/60">
          <Coins className="w-4 h-4 text-[#4fd4f6]" /> Balance {toCredits(depositBalance)} credits
        </span>
      </div>

      <div className="grid grid-cols-2 sm:grid-cols-3 gap-4">
        {packs.map((pk) => {
          // credits_mc already includes the bonus; show the % extra vs the price paid.
          const bonusPct =
            pk.price_cents > 0
              ? Math.round(((pk.credits_mc / 10000 - pk.price_cents) / pk.price_cents) * 100)
              : 0;
          return (
            <div key={pk.id} className="rounded-2xl border border-white/10 bg-white/[0.02] p-5 flex flex-col">
              <div className="text-2xl font-semibold tabular-nums">{fromCents(pk.price_cents)}</div>
              <div className="text-sm text-white/50 mt-1">{toCredits(pk.credits_mc)} credits</div>
              {bonusPct > 0 && (
                <div className="text-[11px] text-[#34d399] mt-1">+{bonusPct}% bonus</div>
              )}
              <button
                onClick={() => onBuy(pk.id)}
                disabled={!pk.purchasable || busy === pk.id}
                className="mt-5 inline-flex items-center justify-center gap-2 rounded-lg border border-white/15 hover:bg-white/5 px-4 py-2 text-sm text-white/80 transition disabled:opacity-40"
              >
                {busy === pk.id ? <Loader2 className="w-4 h-4 animate-spin" /> : <Zap className="w-4 h-4" />}
                Buy
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function SettingsSection({ account, onSignOut }: { account: Account; onSignOut: () => void }) {
  const u = account.user;
  const rows = [
    { label: "Name", value: u.display_name || `${u.first_name} ${u.last_name}`.trim() || "—" },
    { label: "Email", value: u.email },
    { label: "Plan", value: <span className="capitalize">{u.tier}</span> },
    { label: "Member since", value: fmtDate(u.created_at) },
  ];
  return (
    <div className="space-y-6 max-w-2xl">
      <Card>
        <h3 className="font-[family-name:var(--font-outfit)] font-semibold mb-4">Account</h3>
        <div className="flex items-center gap-3 mb-5">
          <Avatar u={u} size={44} />
          <div>
            <div className="font-medium">{u.display_name || u.first_name || u.email}</div>
            <div className="text-sm text-white/40">{u.email}</div>
          </div>
        </div>
        <div className="divide-y divide-white/6">
          {rows.map((r) => (
            <div key={r.label} className="flex items-center justify-between py-3">
              <span className="text-sm text-white/45">{r.label}</span>
              <span className="text-sm">{r.value}</span>
            </div>
          ))}
        </div>
      </Card>

      <div className="flex flex-col sm:flex-row gap-3">
        <a
          href="dragent://"
          className="inline-flex items-center justify-center gap-2 rounded-lg border border-white/10 px-4 py-2.5 text-sm text-white/70 hover:bg-white/5 transition"
        >
          <ArrowLeft className="w-4 h-4" /> Open Dragent app
        </a>
        <button
          onClick={onSignOut}
          className="inline-flex items-center justify-center gap-2 rounded-lg border border-[#f43f5e]/25 text-[#fca5b4] px-4 py-2.5 text-sm hover:bg-[#f43f5e]/10 transition"
        >
          <LogOut className="w-4 h-4" /> Sign out
        </button>
      </div>
    </div>
  );
}

// ── Shared components ─────────────────────────────────────────────────────────

function Center({ children }: { children: React.ReactNode }) {
  return <div className="min-h-[80vh] flex items-center justify-center px-5">{children}</div>;
}

function Card({ children }: { children: React.ReactNode }) {
  return <div className="rounded-2xl border border-white/8 bg-white/[0.02] p-5 sm:p-6">{children}</div>;
}

function StatCard({
  icon,
  label,
  value,
  hint,
  accent,
}: {
  icon: React.ReactNode;
  label: string;
  value: React.ReactNode;
  hint: string;
  accent?: boolean;
}) {
  return (
    <div className={`rounded-2xl border p-5 ${accent ? "ribbon-border" : "border-white/8 bg-white/[0.02]"}`}>
      <div className={`flex items-center gap-2 text-sm mb-3 ${accent ? "text-[#4fd4f6]" : "text-white/40"}`}>
        {icon}
        {label}
      </div>
      <div className="text-2xl font-semibold tabular-nums">{value}</div>
      <div className="text-xs text-white/30 mt-1">{hint}</div>
    </div>
  );
}

function ActionCard({ icon, title, desc, onClick }: { icon: React.ReactNode; title: string; desc: string; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className="group text-left rounded-2xl border border-white/8 bg-white/[0.02] p-5 hover:border-[#19aad8]/30 hover:bg-white/[0.04] transition-colors"
    >
      <div className="inline-flex h-9 w-9 items-center justify-center rounded-lg border border-white/10 bg-white/[0.05] text-[#4fd4f6] mb-3">
        {icon}
      </div>
      <div className="text-sm font-medium">{title}</div>
      <div className="text-xs text-white/40 mt-0.5">{desc}</div>
    </button>
  );
}

function Meter({ label, used, cap, muted }: { label: string; used: number; cap: number; muted?: boolean }) {
  const p = pct(used, cap);
  return (
    <div className="py-2">
      <div className="flex items-center justify-between text-[13px] mb-1.5">
        <span className="text-white/60">{label}</span>
        <span className="text-white/45 tabular-nums">{p}%</span>
      </div>
      <div className="h-2 rounded-full bg-white/[0.06] overflow-hidden">
        <div
          className={`h-full rounded-full ${muted ? "bg-white/25" : "bg-gradient-to-r from-[#2fd4c8] to-[#19aad8]"}`}
          style={{ width: `${p}%` }}
        />
      </div>
    </div>
  );
}

function Avatar({ u, size = 36 }: { u?: Account["user"]; size?: number }) {
  if (u?.avatar_url) {
    // eslint-disable-next-line @next/next/no-img-element
    return <img src={u.avatar_url} alt="" width={size} height={size} className="rounded-full object-cover" style={{ width: size, height: size }} />;
  }
  return (
    <span
      className="inline-flex items-center justify-center rounded-full bg-gradient-to-br from-[#2fd4c8]/30 to-[#6b5bd6]/30 text-[#9be7ff] font-semibold"
      style={{ width: size, height: size, fontSize: size * 0.36 }}
    >
      {initials(u?.display_name ?? "", u?.email ?? "")}
    </span>
  );
}

function TierBadge({ tier }: { tier: string }) {
  const paid = tier !== "free";
  return (
    <span
      className={`text-xs font-medium rounded-full px-3 py-1 border capitalize ${
        paid ? "border-[#19aad8]/40 bg-[#19aad8]/15 text-[#9be7ff]" : "border-white/15 text-white/50"
      }`}
    >
      {tier}
    </span>
  );
}

function IntervalToggle({ value, onChange }: { value: "monthly" | "annual"; onChange: (v: "monthly" | "annual") => void }) {
  return (
    <div className="inline-flex rounded-lg border border-white/10 p-0.5 text-xs">
      {(["monthly", "annual"] as const).map((opt) => (
        <button
          key={opt}
          onClick={() => onChange(opt)}
          className={`px-3 py-1 rounded-md capitalize transition ${
            value === opt ? "bg-[#19aad8]/15 text-white" : "text-white/40 hover:text-white/70"
          }`}
        >
          {opt}
        </button>
      ))}
    </div>
  );
}
