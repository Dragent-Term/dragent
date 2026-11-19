"use client";

import React, { useState, useEffect, Suspense } from "react";
import { useSearchParams, useRouter } from "next/navigation";
import { CheckCircle2, XCircle, Loader2, ArrowRight } from "lucide-react";
import { setToken } from "@/lib/token";
import { stashHandoff } from "@/lib/desktopHandoff";

function MagicVerificationContent() {
  const searchParams = useSearchParams();
  const router = useRouter();

  const [verifying, setVerifying] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  useEffect(() => {
    const token = searchParams.get("token");
    const state = searchParams.get("state");
    const scheme = searchParams.get("scheme") || "dragent";

    if (!token || !state) {
      setError("Invalid link. Missing token or state verification parameters.");
      setVerifying(false);
      return;
    }

    const verifyToken = async () => {
      try {
        const res = await fetch(
          `/api/auth/magic?token=${encodeURIComponent(token)}&state=${encodeURIComponent(state)}`
        );
        const data = await res.json();

        if (!res.ok) {
          throw new Error(data.error || "Verification failed");
        }

        // Clicking a magic link is a full sign-in, so it finishes where every
        // other sign-in finishes: on the dashboard, signed in. It used to fire
        // the desktop deep link and stop there, which left anyone reading their
        // email in a browser at a dead end with no session.
        //
        // Never navigate without an access token. The dashboard authenticates
        // with a Bearer header; landing there tokenless renders "Authentication
        // required" and bounces to /login, which reads as being logged out the
        // instant you log in.
        if (!data.access_token) {
          throw new Error(
            "Signed in, but no access token was issued. Please request a new link."
          );
        }
        setToken(data.access_token);

        // Let the dashboard offer "Open in Dragent". Safe to always stash: the
        // desktop app completes by polling this state, not by the deep link
        // (dragent/app/src/auth/mod.rs), so this is a convenience, not the
        // mechanism — and sessionStorage keeps it to this tab.
        stashHandoff({
          scheme,
          state: data.state || state,
          refresh_token: data.refresh_token,
          user_id: data.user_id ?? "",
        });

        setDone(true);
        router.replace("/dashboard");
      } catch (err: unknown) {
        console.error("Magic link verification error:", err);
        setError((err instanceof Error && err.message) || "The magic link has expired or is invalid. Please try logging in again.");
      } finally {
        setVerifying(false);
      }
    };

    verifyToken();
  }, [searchParams, router]);

  return (
    <div className="relative min-h-screen flex items-center justify-center p-4">
      {/* Background glowing gradients */}
      <div className="absolute top-1/4 left-1/4 -translate-x-1/2 -translate-y-1/2 w-80 h-80 rounded-full bg-brand-cyan/10 blur-[100px] animate-pulse-glow" />
      <div className="absolute bottom-1/4 right-1/4 translate-x-1/2 translate-y-1/2 w-80 h-80 rounded-full bg-emerald-600/10 blur-[100px] animate-pulse-glow" style={{ animationDelay: "1s" }} />

      <div className="relative w-full max-w-md glow-card p-8 rounded-2xl shadow-2xl text-center">
        {/* Header icon */}
        <div className="flex flex-col items-center mb-6">
          <div className="w-16 h-16 rounded-2xl flex items-center justify-center shadow-lg mb-4 overflow-hidden animate-float">
            <img src="/dragent-icon.png" alt="Dragent Logo" className="w-full h-full object-cover" />
          </div>
          <h1 className="text-xl font-bold tracking-tight text-white font-outfit">Magic Link Sign-In</h1>
        </div>

        {verifying ? (
          <div className="py-8 space-y-4">
            <Loader2 className="w-10 h-10 text-brand-cyan animate-spin mx-auto" />
            <p className="text-sm text-gray-400 font-inter">Verifying credentials with the backend server...</p>
          </div>
        ) : error ? (
          <div className="py-4 space-y-6">
            <div className="w-12 h-12 rounded-full bg-brand-rose/10 border border-brand-rose/20 flex items-center justify-center mx-auto">
              <XCircle className="w-6 h-6 text-brand-rose" />
            </div>
            
            <div className="space-y-2">
              <h2 className="text-md font-bold text-white font-outfit">Verification Failed</h2>
              <p className="text-sm text-gray-400 max-w-xs mx-auto leading-relaxed font-inter">{error}</p>
            </div>

            <button
              onClick={() => router.push("/login")}
              className="w-full py-3 bg-white text-black font-semibold rounded-xl hover:bg-white/90 active:bg-white/80 transition-colors flex items-center justify-center text-sm shadow-md"
            >
              Return to Login
              <ArrowRight className="w-4 h-4 ml-1.5" />
            </button>
          </div>
        ) : done ? (
          /* Briefly visible while router.replace lands on /dashboard. The deep
             link and the "Redirect Payload" panel that used to live here are
             gone on purpose: the app signs in by polling, and printing a
             refresh token into the DOM put a live credential in the page for
             any extension or screenshot to read. */
          <div className="py-8 space-y-4">
            <CheckCircle2 className="w-10 h-10 text-brand-emerald mx-auto" />
            <p className="text-sm text-gray-400 font-inter">Signed in. Taking you to your dashboard...</p>
          </div>
        ) : null}
      </div>
    </div>
  );
}

export default function MagicVerificationPage() {
  return (
    <Suspense fallback={
      <div className="min-h-screen flex items-center justify-center bg-[#030303]">
        <Loader2 className="w-8 h-8 text-brand-cyan animate-spin" />
      </div>
    }>
      <MagicVerificationContent />
    </Suspense>
  );
}
