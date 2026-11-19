"use client";

import { useState, useEffect, Suspense } from "react";
import { useParams, useSearchParams, useRouter } from "next/navigation";
import { 
  CheckCircle2, 
  XCircle, 
  Loader2, 
  ArrowRight, 
  ExternalLink 
} from "lucide-react";

function OAuthCallbackContent() {
  const params = useParams();
  const searchParams = useSearchParams();
  const router = useRouter();
  
  const provider = params.provider as string;
  const code = searchParams.get("code");
  const state = searchParams.get("state");

  const [verifying, setVerifying] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [session, setSession] = useState<{
    refresh_token: string;
    state: string;
    user_id?: string;
  } | null>(null);

  useEffect(() => {
    if (!provider || !code || !state) {
      setError("Invalid OAuth callback. Missing code, state, or provider parameters.");
      setVerifying(false);
      return;
    }

    const verifyOAuth = async () => {
      try {
        const res = await fetch(`/api/auth/callback/${provider}?code=${code}&state=${state}`);
        const data = await res.json();
        
        if (!res.ok) {
          throw new Error(data.error || "Authentication failed");
        }

        setSession({
          refresh_token: data.refresh_token,
          state: data.state,
          user_id: data.user_id,
        });

        // Social sign-in runs in a window.open() popup. Hand the result back to
        // the opener and close, instead of leaving the user staring at a stray
        // window with "you can close this tab" on it.
        const opened = Boolean(window.opener) && window.opener !== window;
        if (opened) {
          try {
            // Same-origin: the opener is our own /login page.
            window.opener.postMessage(
              { type: "dragent:oauth-complete", state: data.state },
              window.location.origin
            );
          } catch {
            // Opener gone (user closed it) — the poll on /login still resolves,
            // so closing is still the right move.
          }
          window.close();
          return;
        }

        // Not a popup (provider bounced the whole tab, or popups blocked):
        // fall back to the deep link so the desktop app still gets the token.
        const deepLink = `dragent://auth/callback?refresh_token=${encodeURIComponent(
          data.refresh_token
        )}&state=${encodeURIComponent(data.state)}&user_id=${encodeURIComponent(data.user_id || "")}`;

        setTimeout(() => {
          window.location.href = deepLink;
        }, 1500);

      } catch (err: unknown) {
        console.error("OAuth exchange failed:", err);
        setError((err instanceof Error && err.message) || "Failed to finalize authentication. The server could be offline or the code has expired.");
      } finally {
        setVerifying(false);
      }
    };

    verifyOAuth();
  }, [provider, code, state]);

  const capitalize = (str: string) => str ? str.charAt(0).toUpperCase() + str.slice(1) : "";

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
          <h1 className="text-xl font-bold tracking-tight text-white font-outfit">
            {capitalize(provider)} Login
          </h1>
        </div>

        {verifying ? (
          <div className="py-8 space-y-4">
            <Loader2 className="w-10 h-10 text-brand-cyan animate-spin mx-auto" />
            <p className="text-sm text-gray-400 font-inter">Authorizing access with {capitalize(provider)}...</p>
          </div>
        ) : error ? (
          <div className="py-4 space-y-6">
            <div className="w-12 h-12 rounded-full bg-brand-rose/10 border border-brand-rose/20 flex items-center justify-center mx-auto">
              <XCircle className="w-6 h-6 text-brand-rose" />
            </div>
            
            <div className="space-y-2">
              <h2 className="text-md font-bold text-white font-outfit">Authentication Failed</h2>
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
        ) : session ? (
          <div className="py-4 space-y-6 text-left">
            <div className="flex items-center gap-4 p-4 rounded-xl bg-brand-emerald/10 border border-brand-emerald/20">
              <CheckCircle2 className="w-8 h-8 text-brand-emerald shrink-0 animate-bounce" />
              <div>
                <h2 className="text-md font-bold text-white">Login Authorized!</h2>
                <p className="text-xs text-gray-400">Syncing with desktop client...</p>
              </div>
            </div>

            <p className="text-sm text-gray-400 font-inter text-center">
              Your browser is redirecting you back to the desktop application. You can close this tab afterwards.
            </p>

            <button
              onClick={() => {
                const deepLink = `dragent://auth/callback?refresh_token=${encodeURIComponent(
                  session.refresh_token
                )}&state=${encodeURIComponent(session.state)}&user_id=${encodeURIComponent(session.user_id || "")}`;
                window.location.href = deepLink;
              }}
              className="w-full py-3.5 bg-brand-cyan hover:bg-brand-cyan/90 text-white font-semibold rounded-xl transition-colors flex items-center justify-center gap-2 text-sm shadow-lg shadow-brand-cyan/20"
            >
              <span>Redirect to Desktop App</span>
              <ExternalLink className="w-4 h-4" />
            </button>
            
            <div className="bg-[#080808] border border-white/5 rounded-xl p-4 font-mono text-[11px] text-gray-500">
              <div className="text-gray-400 font-bold mb-1.5 uppercase tracking-wider text-[9px]">Redirect Payload:</div>
              <div className="truncate">Token: {session.refresh_token}</div>
              <div>State: {session.state.substring(0, 16)}...</div>
              <div>Identity: {session.user_id || "authorized"}</div>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}

export default function OAuthCallbackPage() {
  return (
    <Suspense fallback={
      <div className="min-h-screen flex items-center justify-center bg-[#030303]">
        <Loader2 className="w-8 h-8 text-brand-cyan animate-spin" />
      </div>
    }>
      <OAuthCallbackContent />
    </Suspense>
  );
}
