"use client";

import React, { useState, useEffect, Suspense } from "react";
import { useSearchParams, useRouter } from "next/navigation";
import { motion, AnimatePresence } from "framer-motion";
import { ArrowRight, Loader2, Mail, ShieldAlert, ShieldCheck } from "lucide-react";
import SessionControl from "@/components/SessionControl";
import { SIGNED_IN_KEY, markSignedIn } from "@/lib/token";
import { appPart, stashHandoff } from "@/lib/desktopHandoff";

// Custom Google SVG icon
const GoogleIcon = () => (
  <svg className="w-5 h-5 mr-2" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
    <path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z" fill="#4285F4" />
    <path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853" />
    <path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.06H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.94l2.85-2.22.81-.63z" fill="#FBBC05" />
    <path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.06l3.66 2.84c.87-2.6 3.3-4.52 6.16-4.52z" fill="#EA4335" />
  </svg>
);

// Custom GitHub SVG icon
const GithubIcon = () => (
  <svg className="w-5 h-5 mr-2" viewBox="0 0 24 24" fill="currentColor" xmlns="http://www.w3.org/2000/svg">
    <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"/>
  </svg>
);

function LoginDesktopContent() {
  const searchParams = useSearchParams();
  
  // URL Parameters
  const router = useRouter();
  const [scheme, setScheme] = useState("dragent");
  const [state, setState] = useState("");
  const isDesktopFlow = Boolean(searchParams.get("state"));
  // The desktop's PKCE challenge: passed on so the app's half of the sign-in
  // comes back as a one-time code only the app can redeem.
  const challenge = searchParams.get("challenge") || undefined;
  const challengeParam = challenge ? `&challenge=${encodeURIComponent(challenge)}` : "";
  
  // UI State
  const [email, setEmail] = useState("");
  const [firstName, setFirstName] = useState("");
  const [lastName, setLastName] = useState("");
  const [mode, setMode] = useState<"signin" | "signup">("signin");
  const [loading, setLoading] = useState(false);
  const [magicLinkSent, setMagicLinkSent] = useState(false);
  const [error, setError] = useState<string | null>(null);
  
  // Auth Success State
  const [session, setSession] = useState<{
    access_token?: string;
    refresh_token?: string;
    user_id?: string;
  } | null>(null);

  // Polling duration state (5-minute timeout)
  const [pollingTimedOut, setPollingTimedOut] = useState(false);
  
  // Load and check URL parameters
  useEffect(() => {
    const urlScheme = searchParams.get("scheme") || "dragent";
    const urlState = searchParams.get("state");
    
    setScheme(urlScheme);
    
    if (urlState) {
      // Opened BY the desktop app: it owns the state and is waiting on it, so
      // finishing here also completes the app's linking.
      setState(urlState);
    } else {
      // Opened directly in a browser. This is the single login page now, so a
      // missing state is the ordinary case, not an error — mint one and run the
      // same flow. (It used to render a "No Session Available" dead end telling
      // people to go launch the desktop app, which was correct only while this
      // page lived at /login/desktop.)
      setState(
        typeof crypto !== "undefined" && crypto.randomUUID
          ? crypto.randomUUID()
          : `web-${Date.now()}-${Math.random().toString(36).slice(2)}`
      );
    }
  }, [searchParams]);

  // Polling loop for session status check. It only resolves once a sign-in
  // finished elsewhere (a magic link opened on another device) has been
  // approved there; the same-browser cases arrive by message or storage.
  // Not when the desktop app started this sign-in: what's parked then is the
  // app's session, collected once, by the app.
  useEffect(() => {
    if (!state || session || pollingTimedOut || searchParams.get("state")) return;

    const startTime = Date.now();
    const TIMEOUT_LIMIT = 5 * 60 * 1000; // 5 minutes in milliseconds
    const checkSessionStatus = async () => {
      // Check if 5 minutes have elapsed since component mounted / state initialized
      if (Date.now() - startTime > TIMEOUT_LIMIT) {
        setPollingTimedOut(true);
        return;
      }

      try {
        const res = await fetch(`/auth/desktop/session?state=${state}`);
        if (!res.ok) return;
        const data = await res.json();
        
        // A browser-only sign-in finished elsewhere (a magic link opened on
        // another device) and was approved: the route put the session in
        // this browser's cookie. (Desktop-started sign-ins aren't polled
        // here at all; see the guard above.)
        if (data && !data.pending && data.signed_in === true) {
          markSignedIn();
          setSession({ user_id: data.user_id });
          router.replace(safeNext(searchParams.get("next")));
        }
      } catch (err) {
        console.error("Failed checking polling session:", err);
      }
    };

    // Poll every 2 seconds
    const timer = setInterval(checkSessionStatus, 2000);

    return () => clearInterval(timer);
  }, [state, session, pollingTimedOut, searchParams]);

  // The GitHub/Google popup hands its result back here (same origin only).
  useEffect(() => {
    if (!state) return;
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== window.location.origin) return;
      const data = event.data;
      if (!data || data.type !== "dragent:oauth-complete" || data.state !== state) return;
      if (data.signed_in !== true) return;
      markSignedIn();
      if (searchParams.get("state") && (typeof data.handoff_code === "string" || typeof data.refresh_token === "string")) {
        stashHandoff({ scheme, state, ...appPart(data), user_id: data.user_id ?? "" });
      }
      setSession({ user_id: data.user_id });
      router.replace(safeNext(searchParams.get("next")));
    };
    // A magic link opened in another tab of this browser signs that tab in;
    // follow it rather than waiting here.
    const onStorage = (event: StorageEvent) => {
      if (event.key === SIGNED_IN_KEY && event.newValue) {
        router.replace(safeNext(searchParams.get("next")));
      }
    };
    window.addEventListener("message", onMessage);
    window.addEventListener("storage", onStorage);
    return () => {
      window.removeEventListener("message", onMessage);
      window.removeEventListener("storage", onStorage);
    };
  }, [state, scheme, searchParams, router]);

  // Social Login Popup Launcher
  const handleSocialLogin = (provider: "github" | "google") => {
    if (loading) return;
    setError(null);
    
    const width = 600;
    const height = 700;
    const left = window.screenX + (window.outerWidth - width) / 2;
    const top = window.screenY + (window.outerHeight - height) / 2;
    
    // Same-origin proxy route — it redirects to the Go backend. A client
    // component must NOT read NEXT_BACKEND_URL directly: non-NEXT_PUBLIC_ env
    // vars are undefined in the browser bundle, which sent this popup to
    // `<ui-domain>/undefined/auth/start`.
    const popup = window.open(
      `/api/auth/start?provider=${provider}&state=${state}${challengeParam}`,
      `oauth-${provider}`,
      `width=${width},height=${height},left=${left},top=${top},status=no,resizable=yes`
    );

    if (!popup) {
      setError("Popup was blocked by your browser. Please allow popups for this site.");
    }
  };

  // Email Magic Link Submission handler (sign-in mode)
  const handleEmailSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!email.trim() || loading) return;

    setLoading(true);
    setError(null);

    try {
      const res = await fetch("/api/auth/magic", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, state, challenge, is_registration: false }),
      });

      const data = await res.json();

      if (!res.ok) {
        throw new Error(data.error || "Failed to request magic link");
      }

      setMagicLinkSent(true);
    } catch (err: unknown) {
      console.error(err);
      setError((err instanceof Error && err.message) || "Failed to send the magic link. Try again in a moment.");
    } finally {
      setLoading(false);
    }
  };

  // Sign-up form submission handler
  const handleSignupSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!email.trim() || !firstName.trim() || !lastName.trim() || loading) return;

    setLoading(true);
    setError(null);

    try {
      const res = await fetch("/api/auth/magic", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, state, challenge, first_name: firstName, last_name: lastName, is_registration: true }),
      });

      const data = await res.json();
      if (!res.ok) {
        throw new Error(data.error || "Failed to create account");
      }

      setMagicLinkSent(true);
    } catch (err: unknown) {
      console.error(err);
      setError((err instanceof Error && err.message) || "Failed to create the account. Try again in a moment.");
    } finally {
      setLoading(false);
    }
  };

  return (
    <main className="flex min-h-[100svh] flex-col bg-[#030303] px-5 py-7 sm:px-10 sm:py-10">
      <div className="mx-auto flex w-full max-w-sm flex-1 flex-col">
          <div className="mb-10 flex items-center justify-between lg:mb-12">
            <div className="flex items-center gap-2.5 text-sm font-semibold"><img src="/dragent-icon.png" alt="Dragent" className="h-8 w-8 rounded-[24%]" /> Dragent</div>
            <span className="inline-flex items-center gap-1.5 text-xs text-white/45"><ShieldCheck className="h-3.5 w-3.5 text-[#4fd4f6]" /> Secure sign-in</span>
          </div>

          <div className="my-auto w-full rounded-2xl border border-white/10 bg-[#09090b] p-6 sm:p-8">
            <div className="mb-7">
              <h1 className="font-[family-name:var(--font-outfit)] text-3xl font-semibold tracking-tight text-white">{mode === "signup" ? "Create your account" : "Welcome back"}</h1>
              <p className="mt-2 text-sm leading-6 text-white/50">{isDesktopFlow ? "Sign in to securely connect your Dragent desktop app." : mode === "signup" ? "Start managing your work in one secure workspace." : "Sign in to continue to your Dragent workspace."}</p>
            </div>

            {error && (
              <div className="mb-4 flex items-start gap-3 rounded-lg border border-rose-500/20 bg-rose-500/10 p-3 text-xs text-rose-300 sm:mb-6">
                <ShieldAlert className="w-4 h-4 mt-0.5 shrink-0" />
                <div>{error}</div>
              </div>
            )}

            {/* Animated view toggles (Auth Form or Session Control) */}
            <AnimatePresence mode="wait">
              {session ? (
                <motion.div
                  key="session-control"
                  initial={{ opacity: 0, scale: 0.95 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.95 }}
                  transition={{ duration: 0.3 }}
                >
                  <SessionControl 
                    session={session} 
                    scheme={scheme} 
                    state={state} 
                  />
                </motion.div>
              ) : pollingTimedOut ? (
                <motion.div
                  key="polling-timeout"
                  initial={{ opacity: 0, scale: 0.95 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.95 }}
                  transition={{ duration: 0.3 }}
                  className="text-center py-6 space-y-6"
                >
                  <div className="w-12 h-12 rounded-full bg-brand-rose/10 border border-brand-rose/20 flex items-center justify-center mx-auto mb-2">
                    <ShieldAlert className="w-6 h-6 text-brand-rose animate-pulse" />
                  </div>
                  
                  <div className="space-y-2">
                    <h2 className="text-xl font-bold text-white font-outfit">Session Expired</h2>
                    <p className="text-sm text-gray-400 max-w-xs mx-auto leading-relaxed font-inter">
                      For your security, the sign-in handshake timed out after 5 minutes. Start again to get a fresh link.
                    </p>
                  </div>

                  <button
                    onClick={() => {
                      setPollingTimedOut(false);
                      setError(null);
                      window.location.reload();
                    }}
                    className="w-full py-3 bg-brand-cyan hover:bg-brand-cyan/90 active:bg-brand-cyan/85 text-white font-semibold rounded-xl transition-colors flex items-center justify-center gap-2 text-sm"
                  >
                    <span>Restart Sign-In</span>
                  </button>
                </motion.div>
              ) : !magicLinkSent ? (
                <motion.div
                  key={`auth-form-${mode}`}
                  initial={{ opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -10 }}
                  transition={{ duration: 0.2 }}
                  className="space-y-4 sm:space-y-6"
                >
                  {mode === "signin" ? (
                    /* Sign-in: email-only form */
                    <form onSubmit={handleEmailSubmit} className="space-y-3 sm:space-y-4">
                      <div>
                        <label htmlFor="email" className="block text-xs font-semibold text-gray-400 uppercase tracking-wider mb-1.5 font-inter">
                          Email Address
                        </label>
                        <div className="relative">
                          <input
                            id="email"
                            type="email"
                            required
                            placeholder="Enter your email address"
                            value={email}
                            onChange={(e) => setEmail(e.target.value)}
                            className="w-full rounded-xl border border-white/5 bg-[#141414] py-2.5 pl-11 pr-4 text-sm text-white placeholder-gray-500 outline-none transition-all focus:border-brand-cyan/50 focus:ring-1 focus:ring-brand-cyan/30 font-inter sm:py-3"
                          />
                          <Mail className="absolute left-4 top-3.5 w-4.5 h-4.5 text-gray-500" />
                        </div>
                      </div>

                      <button
                        type="submit"
                        disabled={loading || !email}
                        className="flex w-full items-center justify-center rounded-xl bg-white py-2.5 text-sm font-semibold text-black transition-colors hover:bg-white/90 active:bg-white/80 sm:py-3"
                      >
                        {loading ? (
                          <Loader2 className="w-5 h-5 animate-spin" />
                        ) : (
                          <>
                            Continue
                            <ArrowRight className="w-4 h-4 ml-1.5" />
                          </>
                        )}
                      </button>

                      <div className="text-center">
                        <button
                          type="button"
                          onClick={() => { setMode("signup"); setError(null); }}
                          className="text-xs text-gray-400 hover:text-white transition-colors underline underline-offset-4"
                        >
                          Don&apos;t have an account? Sign up
                        </button>
                      </div>
                    </form>
                  ) : (
                    /* Sign-up: first name, last name, email */
                    <form onSubmit={handleSignupSubmit} className="space-y-3 sm:space-y-4">
                      <div className="flex gap-3">
                        <div className="flex-1">
                          <label htmlFor="firstName" className="block text-xs font-semibold text-gray-400 uppercase tracking-wider mb-1.5 font-inter">
                            First Name
                          </label>
                          <input
                            id="firstName"
                            type="text"
                            required
                            placeholder="First"
                            value={firstName}
                            onChange={(e) => setFirstName(e.target.value)}
                            className="w-full rounded-xl border border-white/5 bg-[#141414] px-4 py-2.5 text-sm text-white placeholder-gray-500 outline-none transition-all focus:border-brand-cyan/50 focus:ring-1 focus:ring-brand-cyan/30 font-inter sm:py-3"
                          />
                        </div>
                        <div className="flex-1">
                          <label htmlFor="lastName" className="block text-xs font-semibold text-gray-400 uppercase tracking-wider mb-1.5 font-inter">
                            Last Name
                          </label>
                          <input
                            id="lastName"
                            type="text"
                            required
                            placeholder="Last"
                            value={lastName}
                            onChange={(e) => setLastName(e.target.value)}
                            className="w-full bg-[#141414] border border-white/5 rounded-xl py-3 px-4 text-sm text-white placeholder-gray-500 outline-none focus:border-brand-cyan/50 focus:ring-1 focus:ring-brand-cyan/30 transition-all font-inter"
                          />
                        </div>
                      </div>

                      <div>
                        <label htmlFor="email-signup" className="block text-xs font-semibold text-gray-400 uppercase tracking-wider mb-1.5 font-inter">
                          Email Address
                        </label>
                        <div className="relative">
                          <input
                            id="email-signup"
                            type="email"
                            required
                            placeholder="Enter your email address"
                            value={email}
                            onChange={(e) => setEmail(e.target.value)}
                            className="w-full rounded-xl border border-white/5 bg-[#141414] py-2.5 pl-11 pr-4 text-sm text-white placeholder-gray-500 outline-none transition-all focus:border-brand-cyan/50 focus:ring-1 focus:ring-brand-cyan/30 font-inter sm:py-3"
                          />
                          <Mail className="absolute left-4 top-3.5 w-4.5 h-4.5 text-gray-500" />
                        </div>
                      </div>

                      <button
                        type="submit"
                        disabled={loading || !email || !firstName || !lastName}
                        className="flex w-full items-center justify-center rounded-xl bg-white py-2.5 text-sm font-semibold text-black transition-colors hover:bg-white/90 active:bg-white/80 sm:py-3"
                      >
                        {loading ? (
                          <Loader2 className="w-5 h-5 animate-spin" />
                        ) : (
                          <>
                            Create Account
                            <ArrowRight className="w-4 h-4 ml-1.5" />
                          </>
                        )}
                      </button>

                      <div className="text-center">
                        <button
                          type="button"
                          onClick={() => { setMode("signin"); setError(null); }}
                          className="text-xs text-gray-400 hover:text-white transition-colors underline underline-offset-4"
                        >
                          Already have an account? Sign in
                        </button>
                      </div>
                    </form>
                  )}

                  {/* OR Divider */}
                  <div className="my-4 flex items-center gap-4 sm:my-6">
                    <div className="h-px bg-white/5 flex-1" />
                    <span className="text-[10px] font-bold text-gray-500 tracking-widest uppercase">OR</span>
                    <div className="h-px bg-white/5 flex-1" />
                  </div>

                  {/* Social Logins */}
                  <div className="grid grid-cols-1 gap-3">
                    <button
                      onClick={() => handleSocialLogin("github")}
                      className="flex w-full items-center justify-center rounded-xl border border-white/5 bg-[#18181b] py-2.5 text-sm font-medium text-white transition-all hover:border-white/10 hover:bg-[#202024] active:bg-[#151518] sm:py-3"
                    >
                      <GithubIcon />
                      Continue with GitHub
                    </button>

                    <button
                      onClick={() => handleSocialLogin("google")}
                      className="flex w-full items-center justify-center rounded-xl border border-white/5 bg-[#18181b] py-2.5 text-sm font-medium text-white transition-all hover:border-white/10 hover:bg-[#202024] active:bg-[#151518] sm:py-3"
                    >
                      <GoogleIcon />
                      Continue with Google
                    </button>
                  </div>

                  <div className="mt-6 text-center text-[11px] leading-relaxed text-white/35">
                    By continuing, you agree to our Terms of Service.
                    {isDesktopFlow && <><br />Waiting for your desktop app to connect.</>}
                  </div>
                </motion.div>
              ) : (
                <motion.div
                  key="sent-form"
                  initial={{ opacity: 0, scale: 0.95 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.95 }}
                  transition={{ duration: 0.3 }}
                  className="text-center py-6 space-y-6"
                >
                  <div className="w-16 h-16 rounded-full bg-brand-cyan/10 border border-brand-cyan/20 flex items-center justify-center mx-auto animate-pulse">
                    <Mail className="w-8 h-8 text-brand-cyan" />
                  </div>

                  <div className="space-y-2">
                    <h2 className="text-xl font-bold text-white font-outfit">Check your inbox</h2>
                    <p className="text-sm text-gray-400 max-w-xs mx-auto leading-relaxed font-inter">
                      We sent a magic sign-in link to <span className="text-brand-cyan/85 font-semibold">{email}</span>.{" "}
                      {mode === "signup"
                        ? "We'll create your account when you click the link."
                        : "Click the link to log in."}
                    </p>
                  </div>

                  {/* Loader indicator while polling */}
                  <div className="flex items-center justify-center gap-2 p-3 bg-white/5 rounded-xl max-w-xs mx-auto text-xs text-gray-300 border border-white/5">
                    <Loader2 className="w-4 h-4 text-brand-cyan animate-spin" />
                    <span>Waiting for authorization link click...</span>
                  </div>

                  <button
                    onClick={() => setMagicLinkSent(false)}
                    className="text-xs text-gray-400 hover:text-white transition-colors underline underline-offset-4"
                  >
                    Use a different email or sign-in provider
                  </button>
                </motion.div>
              )}
            </AnimatePresence>
          </div>
      </div>
    </main>
  );
}

/**
 * Where to land after sign-in. Defaults to the dashboard.
 *
 * Only same-origin relative paths are honoured: `next` comes from the URL, so
 * accepting an absolute one would turn this into an open redirect — an attacker
 * could send a real Dragent sign-in link that lands the user on their page.
 * A leading `//` is rejected too, since `//evil.com` is protocol-relative.
 */
function safeNext(next: string | null): string {
  if (!next) return "/dashboard";
  if (!next.startsWith("/") || next.startsWith("//")) return "/dashboard";
  return next;
}

export default function LoginDesktopPage() {
  return (
    <Suspense fallback={
      <div className="min-h-screen flex items-center justify-center bg-[#030303]">
        <Loader2 className="w-8 h-8 text-brand-cyan animate-spin" />
      </div>
    }>
      <LoginDesktopContent />
    </Suspense>
  );
}
