"use client";

import { useEffect, useState } from "react";
import { getToken } from "@/lib/token";

// Mounts the Dragent web viewer — the desktop app compiled to WebAssembly. The
// wasm bundle renders the ENTIRE shared-session UI (navbar, presence, terminal)
// on its own canvas and opens its own WebSocket to the gateway; React only
// resolves the gateway (server-side env, via /api/session/config), hands it to
// the wasm through a window global, and loads the module.
//
// The bundle is loaded via an injected `<script type="module">` rather than a
// bundler import so it works regardless of the build tool, and the wasm reads
// the session/secret straight from the page URL (?session=&secret=).
export default function WasmViewer() {
  const [error, setError] = useState<string | null>(null);
  // The gateway admits view links anonymously but requires a signed-in account
  // for edit links. When an anonymous viewer hits an edit link the wasm calls
  // `window.__DRAGENT_NEEDS_LOGIN__`, and we swap the canvas for the sign-in
  // card instead of the dead "session ended" screen. After sign-in we reload so
  // the wasm reconnects with the token and drops straight into the session.
  const [needsLogin, setNeedsLogin] = useState(false);
  // The wasm bundle is ~19MB, so there is a real wait before anything paints.
  // Without a loading state that wait is a black screen, which reads as broken.
  // `ready` flips when the module has started; the overlay then fades out.
  const [ready, setReady] = useState(false);
  const [phase, setPhase] = useState(0);

  // Advance the status line while loading. These are honest stages of startup,
  // not a fake progress bar: fetch/compile the module, then start the renderer,
  // then open the session socket.
  useEffect(() => {
    if (ready) return;
    const t = setInterval(() => setPhase((p) => Math.min(p + 1, 2)), 2200);
    return () => clearInterval(t);
  }, [ready]);

  useEffect(() => {
    let script: HTMLScriptElement | null = null;
    let cancelled = false;
    // The wasm signals it has taken over the canvas through this hook.
    const gReady = window as unknown as { __DRAGENT_VIEWER_READY__?: () => void };
    gReady.__DRAGENT_VIEWER_READY__ = () => setReady(true);

    // Bridge the wasm's "this session needs a login" signal into React. Set
    // before the module loads so it's ready when the gateway rejects the join.
    const g0 = window as unknown as { __DRAGENT_NEEDS_LOGIN__?: () => void };
    g0.__DRAGENT_NEEDS_LOGIN__ = () => setNeedsLogin(true);

    // The wasm (Rust `log::error!`) writes to console.error; in dev, Next.js's
    // overlay treats ANY console.error as a blocking full-screen error, which
    // hides the canvas. Downgrade Rust-originated logs (they carry a
    // `%cERROR%c <file>` / `app/src/…` / `crates/…` prefix) to console.warn so
    // they don't blank the viewer — genuine React/app errors still surface.
    const origConsoleError = console.error.bind(console);
    console.error = (...args: unknown[]) => {
      const head = typeof args[0] === "string" ? args[0] : "";
      if (
        head.includes("%cERROR%c") ||
        head.includes("app/src/") ||
        head.includes("crates/")
      ) {
        console.warn(...args);
        return;
      }
      origConsoleError(...args);
    };

    // Globals the wasm reads before it starts (it can't read server env or
    // sessionStorage on its own).
    const g = window as unknown as {
      __DRAGENT_GATEWAY__?: string;
      __DRAGENT_TOKEN__?: string;
      __DRAGENT_DISPLAY_NAME__?: string;
    };

    (async () => {
      // These requests are independent, so start them together. The version is
      // a content hash emitted by build-viewer-wasm.sh and cache-busts BOTH the
      // JS binding and its matching wasm binary.
      const gatewayPromise = fetch("/api/session/config").then(async (res) => {
        if (!res.ok) throw new Error("session config unavailable");
        return (await res.json()).gateway as string;
      });
      const versionPromise = fetch("/wasm/version.txt", {
        cache: "no-store",
      }).then(async (res) => {
        if (!res.ok) throw new Error("viewer version unavailable");
        return (await res.text()).trim();
      });

      // Identity (best-effort): a signed-in viewer's token + name so the gateway
      // attaches their real identity/avatar to the presence list. Anonymous
      // link viewers skip this and join as "Viewer", authorised by the secret.
      const identityPromise = (async () => {
        try {
          const token = getToken();
          if (token) {
            g.__DRAGENT_TOKEN__ = token;
            const acc = await fetch("/api/account", {
              headers: { Authorization: `Bearer ${token}` },
            });
            if (acc.ok) {
              const u = (await acc.json())?.user ?? {};
              const name =
                u.display_name ||
                [u.first_name, u.last_name].filter(Boolean).join(" ") ||
                u.email;
              if (name) g.__DRAGENT_DISPLAY_NAME__ = name;
            }
          }
        } catch {
          /* anonymous viewer — no identity */
        }
      })();

      let gateway: string;
      let bundleVersion: string;
      try {
        const loaded = await Promise.all([
          gatewayPromise,
          versionPromise,
          identityPromise,
        ]);
        gateway = loaded[0];
        bundleVersion = loaded[1];
      } catch {
        if (!cancelled) setError("Couldn't load the shared-session viewer.");
        return;
      }
      if (cancelled) return;

      // The viewer renders through wgpu's WebGL backend, which needs a WebGL2
      // context. Some mobile browsers (older iOS Safari, low-memory devices, or
      // hardware-acceleration disabled) can't provide one — without this probe
      // that surfaces only as a silent black canvas. Fail loudly instead.
      try {
        const probe = document.createElement("canvas");
        if (!probe.getContext("webgl2")) {
          setError(
            "This browser can't display the shared session — it doesn't " +
              "support WebGL2. Try Chrome, or a newer browser, and make sure " +
              "hardware acceleration is on.",
          );
          return;
        }
      } catch {
        /* getContext can throw on locked-down browsers — treat as unsupported */
        setError("This browser can't display the shared session (no WebGL2).");
        return;
      }

      // Hand the resolved gateway to the wasm (it can't read server env itself).
      g.__DRAGENT_GATEWAY__ = gateway;

      // Load + start the module. winit hands control to the browser event loop
      // by throwing, so the "control flow" rejection is success, not an error.
      // A *real* init failure (e.g. wgpu couldn't get a device on this phone)
      // would otherwise leave a black screen — surface it on-screen so a mobile
      // viewer, with no console, can see and report what went wrong.
      const version = encodeURIComponent(bundleVersion);
      const jsUrl = `/wasm/dragent_web_viewer.js?v=${version}`;
      const wasmUrl = `/wasm/dragent_web_viewer_bg.wasm?v=${version}`;
      script = document.createElement("script");
      script.type = "module";
      script.textContent = [
        `import init from ${JSON.stringify(jsUrl)};`,
        // winit hands control to the browser event loop by THROWING, so the
        // "control flow" rejection means the app is running — that, and a
        // clean resolve, both count as ready.
        "const ready = () => window.__DRAGENT_VIEWER_READY__ && window.__DRAGENT_VIEWER_READY__();",
        `init({ module_or_path: ${JSON.stringify(wasmUrl)} }).then(ready).catch((e) => {`,
        "  const m = String(e && e.message ? e.message : e);",
        '  if (m.includes("control flow") || m.includes("isn\'t actually an error")) { ready(); return; }',
        '  console.error("[viewer] wasm init failed", e);',
        '  const host = document.getElementById("wasm-container");',
        "  if (host) {",
        '    host.textContent = "Couldn\'t start the session viewer on this device: " + m;',
        '    host.style.cssText += ";display:flex;align-items:center;justify-content:center;padding:24px;color:#9ca3af;font:14px system-ui;text-align:center;";',
        "  }",
        "});",
      ].join("\n");
      document.body.appendChild(script);
    })();

    return () => {
      cancelled = true;
      if (script) script.remove();
      console.error = origConsoleError;
      delete g0.__DRAGENT_NEEDS_LOGIN__;
      delete gReady.__DRAGENT_VIEWER_READY__;
    };
  }, []);

  if (error) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-[#0a0b0e] text-sm text-gray-400 font-inter">
        {error}
      </div>
    );
  }

  // warpui attaches its <canvas> inside #wasm-container (falling back to <body>).
  // The canvas itself must be sized — without an explicit size warpui reads it
  // as 0×0 and renders nothing (a black screen). Style the child canvas, not
  // just the container.
  //
  // The canvas stays mounted even while the sign-in card is shown (removing it
  // would fault the still-running wasm) — the card is just an opaque overlay on
  // top. After sign-in we reload so a fresh wasm connects with the new token.
  return (
    <>
      <style>{`
        html, body {
          width: 100%;
          height: 100%;
          overflow: hidden;
          overscroll-behavior: none;
        }

        /* The container is sized to the viewport, BUT warpui overrides its inline
           height to the visual-viewport height on mobile (its address-bar /
           on-screen-keyboard handling — see handle_visual_viewport_resize). So the
           canvas must FILL THE CONTAINER (height:100%), not the viewport directly:
           if the canvas were 100vh/100dvh it would overflow the (shorter) container
           behind the address bar and the bottom input bar would be clipped
           off-screen — the Chrome-mobile "input not visible" bug. */
        #wasm-container {
          display: block;
          width: 100vw;
          width: 100dvw;
          height: 100vh;
          height: 100dvh;
          outline: none;
        }
        #wasm-container canvas {
          display: block;
          width: 100%;
          height: 100%;
          outline: none;
        }

        /* ── Loading screen ────────────────────────────────────────────── */
        @keyframes dg-breathe {
          0%, 100% { transform: scale(1);    opacity: .55; }
          50%      { transform: scale(1.14); opacity: .95; }
        }
        @keyframes dg-sweep {
          0%   { transform: translateX(-100%); }
          100% { transform: translateX(300%); }
        }
        @keyframes dg-drift {
          0%, 100% { transform: translate3d(0,0,0)      scale(1);   }
          50%      { transform: translate3d(3%, -4%, 0) scale(1.12); }
        }
        @keyframes dg-rise {
          from { opacity: 0; transform: translateY(8px); }
          to   { opacity: 1; transform: translateY(0);   }
        }
        .dg-rise { animation: dg-rise .5s cubic-bezier(.2,.7,.3,1) both; }
        /* Honour the OS setting: hold the final frame instead of animating. */
        @media (prefers-reduced-motion: reduce) {
          .dg-breathe, .dg-sweep, .dg-drift, .dg-rise { animation: none !important; }
        }
      `}</style>
      <div id="wasm-container" className="fixed inset-0 bg-[#0a0b0e]" />

      {/* Branded loading state. Stays mounted (pointer-events off) through the
          fade so the canvas is never revealed mid-paint. */}
      <div
        aria-hidden={ready}
        role="status"
        className={`pointer-events-none fixed inset-0 z-40 flex items-center justify-center
                    bg-[#0a0b0e] transition-opacity duration-700 ease-out
                    ${ready ? "opacity-0" : "opacity-100"}`}
      >
        {/* Ambient wash — two very low-opacity blooms, slowly drifting. */}
        <div
          className="dg-drift absolute h-[46rem] w-[46rem] rounded-full blur-3xl"
          style={{
            background:
              "radial-gradient(circle, rgba(25,170,216,.16) 0%, rgba(25,170,216,0) 65%)",
            animation: "dg-drift 14s ease-in-out infinite",
          }}
        />
        <div
          className="dg-drift absolute h-[30rem] w-[30rem] translate-x-1/3 translate-y-1/3 rounded-full blur-3xl"
          style={{
            background:
              "radial-gradient(circle, rgba(16,185,129,.10) 0%, rgba(16,185,129,0) 65%)",
            animation: "dg-drift 18s ease-in-out infinite reverse",
          }}
        />

        <div className="dg-rise relative flex flex-col items-center px-6 text-center">
          {/* Logo, haloed by a breathing glow. */}
          <div className="relative mb-7 flex h-20 w-20 items-center justify-center">
            <span
              className="dg-breathe absolute inset-0 rounded-[26px] blur-xl"
              style={{
                background: "rgba(25,170,216,.55)",
                animation: "dg-breathe 2.6s ease-in-out infinite",
              }}
            />
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img
              src="/dragent-icon.png"
              alt=""
              width={72}
              height={72}
              className="relative h-[72px] w-[72px] rounded-[20px] ring-1 ring-white/10"
            />
          </div>

          <h1 className="font-[family-name:var(--font-outfit)] text-[28px] font-semibold tracking-tight text-white">
            Dragent
          </h1>
          <p className="mt-1.5 font-[family-name:var(--font-inter)] text-sm text-gray-400">
            Shared terminal session
          </p>

          {/* Indeterminate sweep — deliberately not a percentage, because we
              cannot honestly measure module compile progress. */}
          <div className="mt-8 h-[3px] w-56 overflow-hidden rounded-full bg-white/10">
            <div
              className="dg-sweep h-full w-1/3 rounded-full"
              style={{
                background:
                  "linear-gradient(90deg, transparent, #19aad8, transparent)",
                animation: "dg-sweep 1.5s ease-in-out infinite",
              }}
            />
          </div>

          <p className="mt-4 h-5 font-[family-name:var(--font-inter)] text-[13px] text-gray-500">
            {["Loading viewer…", "Starting renderer…", "Connecting to session…"][phase]}
          </p>
        </div>
      </div>
      {needsLogin && (
        <div className="fixed inset-0 z-50 flex h-[100dvh] w-full items-center justify-center overflow-hidden bg-[#0a0b0e] p-3 sm:p-6">
          <div className="w-full max-w-sm overflow-hidden rounded-2xl border border-white/10 bg-[#0f1013] p-5 text-center sm:p-6">
            <h2 className="font-[family-name:var(--font-outfit)] text-lg font-semibold text-white">
              Sign in to join this session
            </h2>
            <p className="mt-2 text-[13px] leading-relaxed text-white/55">
              This session lets you type — sign in so your commands are
              attributed to you.
            </p>
            {/* One login page, then straight back to this session. */}
            <a
              href={`/login?next=${encodeURIComponent(
                window.location.pathname + window.location.search
              )}`}
              className="mt-5 inline-flex w-full items-center justify-center rounded-xl bg-white px-4 py-2.5 text-[14px] font-medium text-black transition-opacity hover:opacity-90"
            >
              Sign in
            </a>
          </div>
        </div>
      )}
    </>
  );
}
