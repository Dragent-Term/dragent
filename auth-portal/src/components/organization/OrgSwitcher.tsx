"use client";

/**
 * Organization switcher, living at the TOP of the dashboard's persistent
 * sidebar — reachable from every section, not just a dedicated settings
 * page. This placement is deliberate: the reference implementation's org
 * switcher lives in exactly this spot in its app sidebar, and the
 * first version of this feature got that wrong: it put the switcher on a
 * separate /organization page you had to already be on to use, so nothing
 * else in the app ever showed which org you were in or let you leave it.
 */

import { useEffect, useRef, useState } from "react";
import { Building2, Check, ChevronsUpDown, Loader2, Plus } from "lucide-react";
import { apiFetch } from "@/lib/orgApi";

export interface OrgWithRole {
  id: string;
  name: string;
  slug: string;
  role: "owner" | "admin" | "member";
}

const initials = (name: string) => (name.trim()[0] ?? "?").toUpperCase();
const slugify = (s: string) => s.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

export default function OrgSwitcher({
  token,
  active,
  onActiveOrgChanged,
}: {
  token: string;
  /** Controlled from the parent so other sections can react to a switch
   *  without this component owning the only copy of "which org is active." */
  active: OrgWithRole | null;
  onActiveOrgChanged: (org: OrgWithRole) => void;
}) {
  const api = apiFetch(token);
  const [orgs, setOrgs] = useState<OrgWithRole[] | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);

  const loadOrgs = async () => {
    try {
      const list = await api("/api/organization/organizations");
      setOrgs(Array.isArray(list) ? list : []);
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to load organizations");
    }
  };

  useEffect(() => {
    if (token) void loadOrgs();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token]);

  useEffect(() => {
    if (!open) return;
    const onClick = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
        setCreating(false);
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  const switchTo = async (org: OrgWithRole) => {
    if (org.id === active?.id || busy) return;
    setBusy(true);
    setError(null);
    try {
      await api(`/api/organization/organizations/${org.id}/set-active`, { method: "POST" });
      onActiveOrgChanged(org);
      setOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to switch organization");
    } finally {
      setBusy(false);
    }
  };

  const create = async () => {
    const name = newName.trim();
    if (!name || busy) return;
    setBusy(true);
    setError(null);
    try {
      const org = await api("/api/organization/organizations", {
        method: "POST",
        body: JSON.stringify({ name, slug: slugify(name) }),
      });
      setNewName("");
      setCreating(false);
      await loadOrgs();
      await switchTo({ ...org, role: "owner" });
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to create organization");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div ref={rootRef} className="relative mb-4 px-0">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="group flex w-full items-center gap-2.5 rounded-lg border border-white/8 bg-white/[0.02] px-2.5 py-2 text-left transition-colors hover:border-white/15 hover:bg-white/[0.04]"
      >
        <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-gradient-to-br from-[#2fd4c8]/30 to-[#6b5bd6]/30 text-[10px] font-semibold text-[#9be7ff]">
          {active ? initials(active.name) : <Building2 className="h-3.5 w-3.5" />}
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-medium text-white">{active ? active.name : "Loading…"}</span>
          {active && <span className="block truncate text-[10px] capitalize text-white/40">{active.role}</span>}
        </span>
        <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 text-white/30 group-hover:text-white/60" />
      </button>

      {open && (
        <div className="absolute left-0 right-0 top-full z-30 mt-1.5 overflow-hidden rounded-xl border border-white/10 bg-[#0b0b0e] shadow-2xl">
          {error && <div className="border-b border-white/8 px-3 py-2 text-[11px] text-[#fca5b4]">{error}</div>}
          <div className="max-h-64 overflow-y-auto py-1">
            {orgs === null && (
              <div className="flex items-center justify-center py-4">
                <Loader2 className="h-4 w-4 animate-spin text-white/30" />
              </div>
            )}
            {orgs?.map((org) => (
              <button
                key={org.id}
                type="button"
                disabled={busy}
                onClick={() => void switchTo(org)}
                className="flex w-full items-center gap-2.5 px-3 py-2 text-left text-[13px] text-white/75 transition-colors hover:bg-white/[0.05] disabled:opacity-50"
              >
                <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded bg-white/10 text-[9px] font-semibold text-white/70">
                  {initials(org.name)}
                </span>
                <span className="min-w-0 flex-1 truncate">{org.name}</span>
                {org.id === active?.id && <Check className="h-3.5 w-3.5 shrink-0 text-[#4fd4f6]" />}
              </button>
            ))}
          </div>
          <div className="border-t border-white/8 p-1.5">
            {creating ? (
              <div className="flex items-center gap-1.5 p-1">
                <input
                  autoFocus
                  value={newName}
                  onChange={(e) => setNewName(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && void create()}
                  placeholder="Organization name"
                  className="min-w-0 flex-1 rounded-md border border-white/10 bg-white/5 px-2 py-1.5 text-[12px] text-white placeholder:text-white/30 focus:border-[#19aad8]/40 focus:outline-none"
                />
                <button
                  type="button"
                  onClick={() => void create()}
                  disabled={busy || !newName.trim()}
                  className="rounded-md bg-[#19aad8]/20 px-2.5 py-1.5 text-[12px] font-medium text-[#9be7ff] disabled:opacity-40"
                >
                  {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : "Create"}
                </button>
              </div>
            ) : (
              <button
                type="button"
                onClick={() => setCreating(true)}
                className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px] text-white/55 transition-colors hover:bg-white/[0.05] hover:text-white"
              >
                <Plus className="h-3.5 w-3.5" /> Create organization
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
