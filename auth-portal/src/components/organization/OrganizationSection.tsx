"use client";

import React, { useEffect, useMemo, useState } from "react";
import {
  Users,
  Mail,
  Crown,
  Shield,
  UserMinus,
  LogOut,
  Loader2,
  Check,
  X,
  Send,
  CreditCard,
  ExternalLink,
  Lock,
  Trash2,
  FolderPlus,
} from "lucide-react";
import { apiFetch, ApiError } from "@/lib/orgApi";
import type { OrgWithRole } from "@/components/organization/OrgSwitcher";

// ── Types mirroring the Go API (server/internal/organization/handler.go) ──────

interface Member {
  user_id: string;
  email: string;
  role: "owner" | "admin" | "member";
  created_at: number;
}
interface Invitation {
  id: string;
  organization_id: string;
  email: string;
  role: string;
  status: "pending" | "accepted" | "rejected" | "canceled";
  expired: boolean;
  expires_at: number;
  created_at: number;
}
interface Team {
  id: string;
  organization_id: string;
  name: string;
  slug: string;
}
interface OrgBilling {
  enabled: boolean;
  is_paid: boolean;
  plan_id: string;
  tier: string;
  monthly_allowance_mc: number;
  subscription_status: string;
  subscription_renews_at?: string;
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

const fmtDate = (unixSec: number) =>
  unixSec ? new Date(unixSec * 1000).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" }) : "—";
const usd = (cents: number) => (cents / 100).toLocaleString("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 0 });

function Card({ children, className = "" }: { children: React.ReactNode; className?: string }) {
  return <div className={`rounded-2xl border border-white/8 bg-white/[0.02] p-5 sm:p-6 ${className}`}>{children}</div>;
}

/**
 * Free tier is local-only with NO team collaboration at all (invite,
 * teams, domains) — see ORGANIZATION_GAPS_AND_OPEN_QUESTIONS.md §3.1 and
 * the matching backend gate (requireOrgPaid, HTTP 402). Every collaboration
 * surface in this file checks billing.is_paid and shows this instead of a
 * form that would just fail — the UI reflects the real constraint instead
 * of discovering it from a 402 after the fact.
 */
function UpgradeGate({ what, isOwner }: { what: string; isOwner: boolean }) {
  return (
    <div className="flex items-start gap-3 rounded-xl border border-[#19aad8]/20 bg-[#19aad8]/[0.06] px-4 py-3.5 text-sm">
      <Lock className="mt-0.5 h-4 w-4 shrink-0 text-[#4fd4f6]" />
      <div>
        <p className="text-white/80">This organization is on the Free plan, which is local-only.</p>
        <p className="mt-1 text-white/45">
          {isOwner ? `Upgrade to a paid plan to ${what}.` : `Ask an owner to upgrade the plan to ${what}.`}
        </p>
      </div>
    </div>
  );
}

type Tab = "members" | "invitations" | "teams" | "billing";
const TABS: { key: Tab; label: string; icon: typeof Users }[] = [
  { key: "members", label: "Members", icon: Users },
  { key: "invitations", label: "Invitations", icon: Mail },
  { key: "teams", label: "Teams", icon: FolderPlus },
  { key: "billing", label: "Billing", icon: CreditCard },
];

export default function OrganizationSection({ token, org }: { token: string; org: OrgWithRole }) {
  const api = useMemo(() => apiFetch(token), [token]);
  const [tab, setTab] = useState<Tab>("members");
  const [error, setError] = useState<string | null>(null);
  const [billing, setBilling] = useState<OrgBilling | null>(null);

  const isAdmin = org.role === "owner" || org.role === "admin";
  const isOwner = org.role === "owner";

  useEffect(() => {
    setBilling(null);
    api(`/api/organization/organizations/${org.id}/billing`)
      .then(setBilling)
      .catch((e) => setError(e instanceof Error ? e.message : "failed to load billing state"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [org.id]);

  const isPaid = billing?.is_paid ?? false;

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="font-[family-name:var(--font-outfit)] text-xl font-semibold">{org.name}</h2>
          <p className="mt-1 text-sm text-white/45">Manage members, invitations, teams, and billing.</p>
        </div>
        {billing && (
          <span className={`rounded-full px-3 py-1 text-xs font-medium capitalize ${isPaid ? "border border-[#19aad8]/30 bg-[#19aad8]/10 text-[#9be7ff]" : "border border-white/15 text-white/50"}`}>
            {billing.tier} plan
          </span>
        )}
      </div>

      <div className="flex gap-1 overflow-x-auto border-b border-white/8">
        {TABS.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            className={`flex shrink-0 items-center gap-2 border-b-2 px-3 py-2.5 text-sm transition-colors ${tab === t.key ? "border-[#19aad8] text-white" : "border-transparent text-white/45 hover:text-white/75"}`}
          >
            <t.icon className="h-4 w-4" />
            {t.label}
          </button>
        ))}
      </div>

      {error && <div className="rounded-xl border border-[#f43f5e]/30 bg-[#f43f5e]/10 px-4 py-3 text-sm text-[#fca5b4]">{error}</div>}

      {billing === null ? (
        <div className="flex justify-center py-10"><Loader2 className="h-5 w-5 animate-spin text-white/30" /></div>
      ) : (
        <>
          {tab === "members" && <MembersTab api={api} org={org} isAdmin={isAdmin} setError={setError} />}
          {tab === "invitations" && (
            isPaid ? (
              <InvitationsTab api={api} org={org} isAdmin={isAdmin} setError={setError} />
            ) : (
              <UpgradeGate what="invite teammates" isOwner={isOwner} />
            )
          )}
          {tab === "teams" && (
            isPaid ? (
              <TeamsTab api={api} org={org} isAdmin={isAdmin} setError={setError} />
            ) : (
              <UpgradeGate what="create teams" isOwner={isOwner} />
            )
          )}
          {tab === "billing" && <BillingTab api={api} org={org} billing={billing} isOwner={isOwner} setError={setError} />}
        </>
      )}
    </div>
  );
}

// ── Members ───────────────────────────────────────────────────────────────────

function MembersTab({ api, org, isAdmin, setError }: { api: ReturnType<typeof apiFetch>; org: OrgWithRole; isAdmin: boolean; setError: (e: string | null) => void }) {
  const [members, setMembers] = useState<Member[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = async () => {
    try {
      const list = await api(`/api/organization/organizations/${org.id}/members`);
      setMembers(Array.isArray(list) ? list : []);
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to load members");
    }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [org.id]);

  const changeRole = async (userId: string, role: "member" | "admin") => {
    setBusy(userId);
    setError(null);
    try {
      await api(`/api/organization/organizations/${org.id}/members/${userId}`, { method: "PATCH", body: JSON.stringify({ role }) });
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to change role");
    } finally {
      setBusy(null);
    }
  };

  const remove = async (userId: string) => {
    setBusy(userId);
    setError(null);
    try {
      await api(`/api/organization/organizations/${org.id}/members/${userId}`, { method: "DELETE" });
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to remove member");
    } finally {
      setBusy(null);
    }
  };

  const leave = async () => {
    setBusy("__leave__");
    setError(null);
    try {
      await api(`/api/organization/organizations/${org.id}/leave`, { method: "POST" });
      window.location.reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to leave organization");
      setBusy(null);
    }
  };

  if (!members) return <div className="flex justify-center py-10"><Loader2 className="h-5 w-5 animate-spin text-white/30" /></div>;

  return (
    <div className="space-y-4">
      <Card className="overflow-hidden p-0">
        <div className="divide-y divide-white/6">
          {members.map((m) => (
            <div key={m.user_id} className="flex items-center gap-3 px-4 py-3">
              <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-gradient-to-br from-[#2fd4c8]/30 to-[#6b5bd6]/30 text-xs font-semibold text-[#9be7ff]">
                {m.email[0]?.toUpperCase() ?? "?"}
              </span>
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm">{m.email}</div>
                <div className="text-[11px] text-white/40">Joined {fmtDate(m.created_at)}</div>
              </div>
              <RoleBadge role={m.role} />
              {isAdmin && m.role !== "owner" && (
                <div className="flex items-center gap-1.5">
                  <select value={m.role} disabled={busy === m.user_id} onChange={(e) => void changeRole(m.user_id, e.target.value as "member" | "admin")} className="rounded-md border border-white/10 bg-white/5 px-2 py-1 text-[11px] text-white/70 focus:border-[#19aad8]/40 focus:outline-none disabled:opacity-40">
                    <option value="member">Member</option>
                    <option value="admin">Admin</option>
                  </select>
                  <button onClick={() => void remove(m.user_id)} disabled={busy === m.user_id} title="Remove from organization" className="rounded-md p-1.5 text-white/35 transition-colors hover:bg-[#f43f5e]/10 hover:text-[#fca5b4] disabled:opacity-40">
                    {busy === m.user_id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <UserMinus className="h-3.5 w-3.5" />}
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>
      </Card>

      {org.role !== "owner" && (
        <button onClick={() => void leave()} disabled={busy === "__leave__"} className="inline-flex items-center gap-2 rounded-lg border border-[#f43f5e]/25 px-4 py-2 text-sm text-[#fca5b4] transition hover:bg-[#f43f5e]/10 disabled:opacity-50">
          {busy === "__leave__" ? <Loader2 className="h-4 w-4 animate-spin" /> : <LogOut className="h-4 w-4" />}
          Leave organization
        </button>
      )}
    </div>
  );
}

function RoleBadge({ role }: { role: string }) {
  if (role === "owner") return <span className="inline-flex items-center gap-1 rounded-full border border-[#19aad8]/30 bg-[#19aad8]/10 px-2.5 py-1 text-[11px] font-medium text-[#9be7ff]"><Crown className="h-3 w-3" /> Owner</span>;
  if (role === "admin") return <span className="inline-flex items-center gap-1 rounded-full border border-white/15 px-2.5 py-1 text-[11px] font-medium text-white/60"><Shield className="h-3 w-3" /> Admin</span>;
  return <span className="rounded-full border border-white/10 px-2.5 py-1 text-[11px] text-white/40">Member</span>;
}

// ── Invitations ───────────────────────────────────────────────────────────────

function InvitationsTab({ api, org, isAdmin, setError }: { api: ReturnType<typeof apiFetch>; org: OrgWithRole; isAdmin: boolean; setError: (e: string | null) => void }) {
  const [invitations, setInvitations] = useState<Invitation[] | null>(null);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<"member" | "admin">("member");
  const [busy, setBusy] = useState<string | null>(null);

  const load = async () => {
    if (!isAdmin) { setInvitations([]); return; }
    try {
      const list = await api(`/api/organization/organizations/${org.id}/invitations`);
      setInvitations(Array.isArray(list) ? list : []);
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to load invitations");
    }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [org.id, isAdmin]);

  const invite = async () => {
    const trimmed = email.trim();
    if (!trimmed || busy) return;
    setBusy("__invite__");
    setError(null);
    try {
      await api(`/api/organization/organizations/${org.id}/invitations`, { method: "POST", body: JSON.stringify({ email: trimmed, role }) });
      setEmail("");
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to send invitation");
    } finally {
      setBusy(null);
    }
  };

  const cancel = async (id: string) => {
    setBusy(id);
    setError(null);
    try {
      await api(`/api/organization/organizations/${org.id}/invitations/${id}/cancel`, { method: "POST" });
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to cancel invitation");
    } finally {
      setBusy(null);
    }
  };

  if (!isAdmin) {
    return <p className="py-4 text-sm text-white/40">Only an admin or owner can invite members or view sent invitations.</p>;
  }

  return (
    <div className="space-y-4">
      <Card>
        <div className="flex flex-col gap-2 sm:flex-row">
          <input value={email} onChange={(e) => setEmail(e.target.value)} onKeyDown={(e) => e.key === "Enter" && void invite()} placeholder="teammate@company.com" type="email" className="min-w-0 flex-1 rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-white placeholder:text-white/30 focus:border-[#19aad8]/40 focus:outline-none" />
          <select value={role} onChange={(e) => setRole(e.target.value as "member" | "admin")} className="rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-white/70 focus:border-[#19aad8]/40 focus:outline-none">
            <option value="member">Member</option>
            <option value="admin">Admin</option>
          </select>
          <button onClick={() => void invite()} disabled={busy === "__invite__" || !email.trim()} className="inline-flex items-center justify-center gap-2 rounded-lg bg-gradient-to-r from-[#19aad8] to-[#3f7fe0] px-4 py-2 text-sm font-medium text-white transition disabled:opacity-40">
            {busy === "__invite__" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
            Invite
          </button>
        </div>
      </Card>

      {!invitations ? (
        <div className="flex justify-center py-6"><Loader2 className="h-5 w-5 animate-spin text-white/30" /></div>
      ) : invitations.length === 0 ? (
        <p className="py-4 text-sm text-white/40">No invitations sent yet.</p>
      ) : (
        <Card className="overflow-hidden p-0">
          <div className="divide-y divide-white/6">
            {invitations.map((inv) => (
              <div key={inv.id} className="flex items-center gap-3 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm">{inv.email}</div>
                  <div className="text-[11px] capitalize text-white/40">{inv.role} · {inv.status === "pending" && inv.expired ? "expired" : inv.status}</div>
                </div>
                {inv.status === "pending" && !inv.expired && (
                  <button onClick={() => void cancel(inv.id)} disabled={busy === inv.id} className="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-[12px] text-white/45 transition-colors hover:bg-white/5 hover:text-white/80 disabled:opacity-40">
                    {busy === inv.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <X className="h-3.5 w-3.5" />}
                    Cancel
                  </button>
                )}
              </div>
            ))}
          </div>
        </Card>
      )}
    </div>
  );
}

/** Pending invitations addressed to the CALLER, shown above the dashboard
 *  regardless of which org is active — an invitation to a DIFFERENT org
 *  wouldn't otherwise be reachable from wherever the caller happens to be. */
export function MyInvitationsBanner({ token, onAccepted }: { token: string; onAccepted: () => void }) {
  const api = useMemo(() => apiFetch(token), [token]);
  const [invitations, setInvitations] = useState<Invitation[]>([]);
  const [busy, setBusy] = useState<string | null>(null);

  const load = async () => {
    try {
      const list = await api("/api/organization/me/invitations");
      setInvitations(Array.isArray(list) ? list : []);
    } catch { /* non-fatal — dismissible banner, not core content */ }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, []);

  const respond = async (id: string, action: "accept" | "reject") => {
    setBusy(id);
    try {
      await api(`/api/organization/invitations/${id}/${action}`, { method: "POST" });
      if (action === "accept") onAccepted();
      await load();
    } catch (e) {
      // A stale invite (org downgraded since it was sent) 402s here —
      // surface it rather than silently doing nothing.
      if (e instanceof ApiError) await load();
    } finally {
      setBusy(null);
    }
  };

  if (invitations.length === 0) return null;

  return (
    <div className="space-y-2">
      {invitations.map((inv) => (
        <div key={inv.id} className="flex items-center gap-3 rounded-xl border border-[#19aad8]/25 bg-[#19aad8]/10 px-4 py-3">
          <Mail className="h-4 w-4 shrink-0 text-[#4fd4f6]" />
          <p className="min-w-0 flex-1 truncate text-sm text-white/80">
            You&apos;ve been invited to join an organization as <span className="capitalize">{inv.role}</span>.
          </p>
          <button onClick={() => void respond(inv.id, "accept")} disabled={busy === inv.id} className="inline-flex items-center gap-1.5 rounded-md bg-[#19aad8]/20 px-3 py-1.5 text-[12px] font-medium text-[#9be7ff] disabled:opacity-40">
            {busy === inv.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Check className="h-3.5 w-3.5" />}
            Accept
          </button>
          <button onClick={() => void respond(inv.id, "reject")} disabled={busy === inv.id} className="inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-[12px] text-white/50 hover:text-white/80 disabled:opacity-40">
            Decline
          </button>
        </div>
      ))}
    </div>
  );
}

// ── Teams ─────────────────────────────────────────────────────────────────────

function TeamsTab({ api, org, isAdmin, setError }: { api: ReturnType<typeof apiFetch>; org: OrgWithRole; isAdmin: boolean; setError: (e: string | null) => void }) {
  const [teams, setTeams] = useState<Team[] | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  const slugify = (s: string) => s.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

  const load = async () => {
    try {
      const list = await api(`/api/organization/organizations/${org.id}/teams`);
      setTeams(Array.isArray(list) ? list : []);
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to load teams");
    }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [org.id]);

  const create = async () => {
    const trimmed = name.trim();
    if (!trimmed || busy) return;
    setBusy("__create__");
    setError(null);
    try {
      await api(`/api/organization/organizations/${org.id}/teams`, { method: "POST", body: JSON.stringify({ name: trimmed, slug: slugify(trimmed) }) });
      setName("");
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to create team");
    } finally {
      setBusy(null);
    }
  };

  const remove = async (teamID: string) => {
    setBusy(teamID);
    setError(null);
    try {
      await api(`/api/organization/organizations/${org.id}/teams/${teamID}`, { method: "DELETE" });
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to delete team");
    } finally {
      setBusy(null);
    }
  };

  if (!teams) return <div className="flex justify-center py-10"><Loader2 className="h-5 w-5 animate-spin text-white/30" /></div>;

  return (
    <div className="space-y-4">
      {isAdmin && (
        <Card>
          <div className="flex flex-col gap-2 sm:flex-row">
            <input value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && void create()} placeholder="Team name" className="min-w-0 flex-1 rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-white placeholder:text-white/30 focus:border-[#19aad8]/40 focus:outline-none" />
            <button onClick={() => void create()} disabled={busy === "__create__" || !name.trim()} className="inline-flex items-center justify-center gap-2 rounded-lg bg-gradient-to-r from-[#19aad8] to-[#3f7fe0] px-4 py-2 text-sm font-medium text-white transition disabled:opacity-40">
              {busy === "__create__" ? <Loader2 className="h-4 w-4 animate-spin" /> : <FolderPlus className="h-4 w-4" />}
              Create
            </button>
          </div>
        </Card>
      )}
      {teams.length === 0 ? (
        <p className="py-4 text-sm text-white/40">No teams yet.</p>
      ) : (
        <Card className="overflow-hidden p-0">
          <div className="divide-y divide-white/6">
            {teams.map((t) => (
              <div key={t.id} className="flex items-center gap-3 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <div className="text-sm">{t.name}</div>
                  <div className="text-[11px] text-white/40">{t.slug}</div>
                </div>
                {isAdmin && (
                  <button onClick={() => void remove(t.id)} disabled={busy === t.id} className="rounded-md p-1.5 text-white/35 transition-colors hover:bg-[#f43f5e]/10 hover:text-[#fca5b4] disabled:opacity-40">
                    {busy === t.id ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Trash2 className="h-3.5 w-3.5" />}
                  </button>
                )}
              </div>
            ))}
          </div>
        </Card>
      )}
      <p className="text-xs text-white/35">Team member management is available from each team once created.</p>
    </div>
  );
}

// ── Billing ───────────────────────────────────────────────────────────────────

function BillingTab({
  api, org, billing, isOwner, setError,
}: {
  api: ReturnType<typeof apiFetch>; org: OrgWithRole; billing: OrgBilling; isOwner: boolean;
  setError: (e: string | null) => void;
}) {
  const [plans, setPlans] = useState<Plan[]>([]);
  const [billingInterval, setBillingInterval] = useState<"monthly" | "annual">("monthly");
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    api("/api/billing/plans").then((pl) => setPlans(pl.plans ?? [])).catch((e) => setError(e instanceof Error ? e.message : "failed to load plans"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const subscribe = async (planId: string) => {
    setBusy(planId);
    setError(null);
    try {
      const { url } = await api(`/api/organization/organizations/${org.id}/billing/subscribe`, { method: "POST", body: JSON.stringify({ plan_id: planId }) });
      window.location.assign(url);
    } catch (e) {
      setError(e instanceof Error ? e.message : "checkout failed");
      setBusy(null);
    }
  };

  if (!billing.enabled) {
    return <p className="py-4 text-sm text-white/40">Organization billing isn&apos;t available on this deployment right now.</p>;
  }

  if (billing.is_paid) {
    return (
      <div className="max-w-lg space-y-4">
        <Card>
          <div className="flex items-center justify-between">
            <div>
              <div className="text-xs text-white/40">Current plan</div>
              <div className="mt-1 text-2xl font-semibold capitalize">{billing.tier}</div>
            </div>
            <span className="rounded-full border border-[#19aad8]/30 bg-[#19aad8]/10 px-3 py-1 text-xs font-medium capitalize text-[#9be7ff]">{billing.subscription_status || "active"}</span>
          </div>
          {billing.subscription_renews_at && <p className="mt-3 text-xs text-white/40">Renews {fmtDate(new Date(billing.subscription_renews_at).getTime() / 1000)}</p>}
          <p className="mt-4 text-xs leading-relaxed text-white/40">
            Every member gets their own individual AI-credit allowance at this tier automatically as
            they join or leave. Changing the organization&apos;s plan isn&apos;t available yet —
            contact support if you need a different tier.
          </p>
        </Card>
      </div>
    );
  }

  if (!isOwner) {
    return <p className="py-4 text-sm text-white/40">This organization is on the free plan. Ask an owner to upgrade — only the owner can start a subscription.</p>;
  }

  const shown = plans.filter((p) => p.purchasable && p.billing_interval === billingInterval);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-white/45">Every current and future member becomes a billed seat at this tier automatically.</p>
        <div className="inline-flex rounded-lg border border-white/10 p-0.5 text-xs">
          {(["monthly", "annual"] as const).map((opt) => (
            <button key={opt} onClick={() => setBillingInterval(opt)} className={`rounded-md px-3 py-1 capitalize transition ${billingInterval === opt ? "bg-[#19aad8]/15 text-white" : "text-white/40 hover:text-white/70"}`}>{opt}</button>
          ))}
        </div>
      </div>
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {shown.map((p) => (
          <Card key={p.id}>
            <h3 className="font-medium capitalize">{p.tier}</h3>
            <div className="mt-3 mb-5">
              <span className="text-3xl font-semibold tabular-nums">{usd(p.billing_interval === "annual" ? p.price_cents / 12 : p.price_cents)}</span>
              <span className="text-sm text-white/40"> / seat / mo</span>
            </div>
            <button onClick={() => void subscribe(p.id)} disabled={busy === p.id} className="mt-auto inline-flex w-full items-center justify-center gap-2 rounded-lg bg-white/10 px-4 py-2 text-sm font-medium text-white transition hover:bg-white/15 disabled:opacity-40">
              {busy === p.id ? <Loader2 className="h-4 w-4 animate-spin" /> : <ExternalLink className="h-4 w-4" />}
              Subscribe
            </button>
          </Card>
        ))}
      </div>
    </div>
  );
}
