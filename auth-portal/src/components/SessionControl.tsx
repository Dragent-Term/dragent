"use client";

import React, { useState, useEffect } from "react";
import { signInCode } from "@/lib/signInCode";
import { CheckCircle2, ExternalLink } from "lucide-react";
import { appDeepLink } from "@/lib/desktopHandoff";

interface SessionControlProps {
  session: {
    access_token?: string;
    /** One-time code for the app (desktop builds with PKCE). */
    code?: string;
    /** The app's refresh token (desktop builds from before PKCE). */
    refresh_token?: string;
    user_id?: string;
  };
  scheme: string;
  state: string;
}

export default function SessionControl({ session, scheme, state }: SessionControlProps) {
  const [code, setCode] = useState("");
  const [approval, setApproval] = useState<"idle" | "busy" | "done" | "error">("idle");
  const [approvalError, setApprovalError] = useState("");
  useEffect(() => {
    if (state) signInCode(state).then(setCode).catch(() => setCode(""));
  }, [state]);

  // The app collects its session by polling only after this approval, and
  // only the signed-in user can give it. Approving is safe only if the app
  // on the user's own screen shows the same code: someone who sent them a
  // sign-in link would be the one waiting otherwise.
  const approve = async () => {
    setApproval("busy");
    setApprovalError("");
    try {
      const res = await fetch("/api/auth/desktop/approve", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ state }),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.error || "Couldn't approve. Try again.");
      setApproval("done");
    } catch (err: unknown) {
      setApproval("error");
      setApprovalError(err instanceof Error ? err.message : "Couldn't approve. Try again.");
    }
  };

  const refreshToken = session.refresh_token || "";
  const userId = session.user_id || "unknown-user-id";

  // The link that hands the sign-in to the app: a one-time code the app
  // redeems with its own secret, or (older builds) the refresh token.
  const deepLinkUrl = appDeepLink({ state, code: session.code, refresh_token: refreshToken, user_id: userId });

  // Open the app on its own shortly after sign-in; the button below does it again.
  useEffect(() => {
    const launch = setTimeout(() => {
      window.location.href = deepLinkUrl;
    }, 1500);
    return () => clearTimeout(launch);
  }, [deepLinkUrl]);

  const triggerDeepLink = () => {
    window.location.href = deepLinkUrl;
  };

  return (
    <div className="space-y-6 text-left">
      {/* Success banner */}
      <div className="flex items-center gap-4 p-4 rounded-xl bg-brand-emerald/10 border border-brand-emerald/20">
        <CheckCircle2 className="w-8 h-8 text-brand-emerald shrink-0 animate-bounce" />
        <div>
          <h2 className="text-md font-bold text-white">Sign In Successful!</h2>
          <p className="text-xs text-gray-400">Your session is authenticated and ready to link.</p>
        </div>
      </div>

      {/* Approve the waiting app, after matching its code */}
      {code && (
        <div className="rounded-xl border border-white/10 bg-[#101010] p-4 space-y-3">
          <p className="text-xs text-gray-400">
            Is the Dragent app on your screen showing this code? Approve only if it is.
            If you didn&apos;t start this sign-in, don&apos;t approve.
          </p>
          <div className="text-center font-mono text-2xl tracking-[0.3em] text-white select-all">{code}</div>
          {approval === "done" ? (
            <p className="text-center text-xs text-brand-emerald">Approved. The app will finish signing in.</p>
          ) : (
            <button
              onClick={approve}
              disabled={approval === "busy"}
              className="w-full py-2.5 bg-white text-black font-semibold rounded-lg transition-colors hover:bg-white/90 text-sm disabled:opacity-60"
            >
              {approval === "busy" ? "Approving..." : "The codes match, approve"}
            </button>
          )}
          {approval === "error" && <p className="text-center text-xs text-rose-300">{approvalError}</p>}
        </div>
      )}

      {/* Connection options */}
      <div className="space-y-3">
        <button
          onClick={triggerDeepLink}
          className="w-full py-3.5 bg-brand-cyan hover:bg-brand-cyan/90 active:bg-brand-cyan/85 text-white font-semibold rounded-xl transition-colors flex items-center justify-center gap-2 text-sm shadow-lg shadow-brand-cyan/20"
        >
          <span>Launch Desktop App</span>
          <ExternalLink className="w-4 h-4" />
        </button>
      </div>
    </div>
  );
}
