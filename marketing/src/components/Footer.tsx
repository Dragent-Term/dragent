"use client";

import Link from "next/link";
import { Globe, Monitor, Moon, Sun } from "lucide-react";
import { useSyncExternalStore } from "react";
import { Logo } from "./Logo";
import { site } from "@/lib/site";

type ThemeMode = "system" | "light" | "dark";

const themeOptions: Array<{
  mode: ThemeMode;
  label: string;
  icon: typeof Monitor;
}> = [
  { mode: "system", label: "System", icon: Monitor },
  { mode: "light", label: "Light", icon: Sun },
  { mode: "dark", label: "Dark", icon: Moon },
];

const themeChangeEvent = "dragent-theme-change";

function storedTheme(): ThemeMode {
  const value = document.documentElement.dataset.theme;
  return value === "light" || value === "dark" || value === "system"
    ? value
    : "dark";
}

function subscribeToTheme(onStoreChange: () => void) {
  window.addEventListener("storage", onStoreChange);
  window.addEventListener(themeChangeEvent, onStoreChange);
  return () => {
    window.removeEventListener("storage", onStoreChange);
    window.removeEventListener(themeChangeEvent, onStoreChange);
  };
}

function applyTheme(mode: ThemeMode) {
  document.documentElement.dataset.theme = mode;
  try {
    window.localStorage.setItem("dragent-theme", mode);
  } catch {
    /* Theme still applies when storage is blocked. */
  }
  window.dispatchEvent(new Event(themeChangeEvent));
}

const columns = [
  {
    title: "Product",
    links: [
      { label: "Product", href: "/#product" },
      { label: "Agent coordination", href: "/docs/coordination" },
      { label: "Pricing", href: "/pricing" },
      { label: "Download", href: "/download" },
    ],
  },
  {
    title: "Account",
    links: [
      { label: "Sign in", href: site.appUrl },
      { label: "Quickstart", href: "/docs/quickstart" },
      { label: "Docs", href: site.docsUrl },
    ],
  },
  {
    title: "Legal",
    links: [
      { label: "Terms of Service", href: "/terms" },
      { label: "Privacy Policy", href: "/privacy" },
      { label: "Refund Policy", href: "/refund" },
      { label: "Contact", href: `mailto:${site.contactEmail}` },
    ],
  },
];

export function Footer() {
  const theme = useSyncExternalStore(
    subscribeToTheme,
    storedTheme,
    () => "dark",
  );

  function changeTheme(mode: ThemeMode) {
    applyTheme(mode);
  }

  return (
    <footer className="relative z-10 border-t border-theme-fg/8 bg-surface">
      <div className="mx-auto max-w-6xl px-5 py-14 sm:px-8">
        <div className="grid gap-10 lg:grid-cols-[1.4fr_2fr]">
          <div className="max-w-xs">
            <Logo />
            <p className="mt-4 text-sm leading-relaxed text-theme-fg/65">
              A connected workspace for coding agents, your terminal, and the
              work you ship.
            </p>
          </div>
          {/* Link groups: 2 columns on phones, 3 across on tablet and up. */}
          <div className="grid grid-cols-2 gap-8 sm:grid-cols-3">
            {columns.map((col) => (
              <div key={col.title}>
                <h4 className="text-xs font-semibold uppercase tracking-[0.14em] text-theme-fg/60">
                  {col.title}
                </h4>
                <ul className="mt-4 space-y-2.5">
                  {col.links.map((link) => (
                    <li key={link.label}>
                      <Link
                        href={link.href}
                        className="text-sm text-theme-fg/60 transition-colors hover:text-theme-fg"
                      >
                        {link.label}
                      </Link>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        </div>

        <div className="mt-12 flex flex-col items-start justify-between gap-5 border-t border-theme-fg/8 pt-6 sm:flex-row sm:items-center">
          <p className="text-xs text-theme-fg/60">
            © {new Date().getFullYear()} {site.legalName}. Licensed under the{" "}
            <Link
              href="/licenses"
              className="underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
            >
              AGPL-3.0-only
            </Link>
            .
          </p>
          <div className="flex flex-col items-start gap-4 sm:flex-row sm:items-center sm:gap-6">
            <p className="text-xs text-theme-fg/60">
              Payments processed securely by our payment provider.
            </p>
            <div className="flex items-center gap-3">
              <div
                aria-label="Color theme"
                className="inline-flex items-center rounded-full border border-theme-fg/10 bg-theme-fg/[0.06] p-1"
                role="group"
              >
                {themeOptions.map(({ mode, label, icon: Icon }) => (
                  <button
                    key={mode}
                    type="button"
                    aria-label={`${label} theme`}
                    aria-pressed={theme === mode}
                    onClick={() => changeTheme(mode)}
                    className={`inline-flex min-h-9 min-w-9 items-center justify-center rounded-full px-2.5 text-xs transition-colors ${
                      theme === mode
                        ? "bg-theme-fg/[0.14] text-theme-fg shadow-sm"
                        : "text-theme-fg/65 hover:bg-theme-fg/[0.08] hover:text-theme-fg/80"
                    }`}
                  >
                    <Icon size={15} strokeWidth={1.8} aria-hidden="true" />
                    <span className="sr-only">{label}</span>
                  </button>
                ))}
              </div>
              <span
                aria-label="Language: English"
                className="inline-flex min-h-10 items-center gap-2 rounded-full border border-theme-fg/10 bg-theme-fg/[0.06] px-3 text-sm text-theme-fg/75 transition-colors hover:bg-theme-fg/[0.1] hover:text-theme-fg"
              >
                <Globe size={15} strokeWidth={1.8} aria-hidden="true" />
                <span>English</span>
              </span>
            </div>
          </div>
        </div>
      </div>
    </footer>
  );
}
