"use client";

import { useState } from "react";
import Link from "next/link";
import { Check } from "lucide-react";
import { site } from "@/lib/site";

type Tier = {
  name: string;
  blurb: string;
  monthly: number; // dollars/mo
  featured?: boolean;
  cta: string;
  features: string[];
};

const TIERS: Tier[] = [
  {
    name: "Free",
    blurb: "Start with the workspace and built-in agent.",
    monthly: 0,
    cta: "Download",
    features: [
      "Full block-based terminal",
      "AI fix on failed commands",
      "Bring your own API keys",
      "A monthly usage allowance, used at any pace",
    ],
  },
  {
    name: "Pro",
    blurb: "For regular work with the built-in agent.",
    monthly: 20,
    featured: true,
    cta: "Start Pro",
    features: [
      "$20 of agent usage / month",
      "Access to supported built-in agent models",
      "Share a live session — view or full control",
      "Use the allowance at your own pace",
    ],
  },
  {
    name: "Max",
    blurb: "More room for larger agent tasks.",
    monthly: 60,
    cta: "Start Max",
    features: [
      "$60 of agent usage / month",
      "Everything in Pro",
      "Same model access as Pro",
      "More included monthly usage",
    ],
  },
  {
    name: "Ultra",
    blurb: "More included usage for demanding projects.",
    monthly: 200,
    cta: "Start Ultra",
    features: [
      "$200 of agent usage / month",
      "Everything in Max",
      "Use your full allowance at any pace",
      "Manage billing in your account",
    ],
  },
];

function priceLabel(tier: Tier, annual: boolean): string {
  if (tier.monthly === 0) return "$0";
  const perMonth = annual ? Math.round(tier.monthly * 0.9) : tier.monthly;
  return `$${perMonth}`;
}

export function Pricing() {
  const [annual, setAnnual] = useState(true);

  return (
    <div>
      {/* billing toggle */}
      <div className="mb-10 flex items-center justify-center">
        <div className="inline-flex items-center gap-1 rounded-full border border-theme-fg/10 bg-theme-fg/[0.03] p-1">
          {(["monthly", "annual"] as const).map((mode) => {
            const active = (mode === "annual") === annual;
            return (
              <button
                key={mode}
                type="button"
                aria-pressed={active}
                onClick={() => setAnnual(mode === "annual")}
                className={`relative rounded-full px-4 py-1.5 text-sm font-medium transition-colors ${
                  active
                    ? "text-canvas"
                    : "text-theme-fg/55 hover:text-theme-fg"
                }`}
              >
                {active && (
                  <span className="absolute inset-0 rounded-full bg-theme-fg" />
                )}
                <span className="relative capitalize">{mode}</span>
                {mode === "annual" && (
                  <span
                    className={`relative ml-1.5 text-[11px] ${
                      active ? "text-canvas/60" : "text-theme-fg/55"
                    }`}
                  >
                    −10%
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>

      <div className="grid gap-5 lg:grid-cols-4">
        {TIERS.map((tier) => (
          <div
            key={tier.name}
            className={`relative flex flex-col rounded-2xl p-6 ${
              tier.featured
                ? "ribbon-border shadow-[0_24px_70px_-30px_rgba(25,170,216,0.5)]"
                : "hairline bg-theme-fg/[0.02]"
            }`}
          >
            {tier.featured && (
              <span className="absolute -top-3 left-6 rounded-full bg-gradient-to-r from-[#2fd4c8] to-[#19aad8] px-2.5 py-0.5 text-[11px] font-semibold text-[#05070a]">
                Recommended
              </span>
            )}

            <h3 className="font-[family-name:var(--font-outfit)] text-lg font-semibold text-theme-fg">
              {tier.name}
            </h3>
            <p className="mt-1 min-h-[40px] text-[13px] leading-snug text-theme-fg/65">
              {tier.blurb}
            </p>

            <div className="mt-4 flex items-baseline gap-1.5">
              <span className="font-[family-name:var(--font-outfit)] text-4xl font-semibold tracking-tight text-theme-fg tabular-nums">
                {priceLabel(tier, annual)}
              </span>
              <span className="text-sm text-theme-fg/60">
                {tier.monthly === 0 ? "forever" : "/ mo"}
              </span>
            </div>
            <p className="mt-1 h-4 text-[11px] text-theme-fg/60">
              {annual && tier.monthly > 0
                ? `billed $${Math.round(tier.monthly * 0.9 * 12)} yearly`
                : " "}
            </p>

            <Link
              href={tier.monthly === 0 ? "/download" : site.appUrl}
              className={`mt-5 inline-flex items-center justify-center rounded-lg px-4 py-2.5 text-sm font-medium transition-transform duration-200 hover:-translate-y-px ${
                tier.featured
                  ? "bg-gradient-to-r from-[#087d9c] to-[#3f55b5] text-white shadow-[0_8px_24px_-8px_rgba(25,170,216,0.7)]"
                  : "border border-theme-fg/12 bg-theme-fg/[0.02] text-theme-fg hover:border-theme-fg/25"
              }`}
            >
              {tier.cta}
            </Link>

            <ul className="mt-6 space-y-3 border-t border-theme-fg/8 pt-6">
              {tier.features.map((f) => (
                <li
                  key={f}
                  className="flex items-start gap-2.5 text-[13px] text-theme-fg/70"
                >
                  <Check
                    size={15}
                    className={`mt-0.5 shrink-0 ${
                      tier.featured ? "text-accent" : "text-theme-fg/60"
                    }`}
                  />
                  {f}
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>

      {/* top-ups */}
      <div className="mt-6 flex flex-col items-center justify-between gap-3 rounded-2xl border border-theme-fg/8 bg-theme-fg/[0.02] px-6 py-5 sm:flex-row">
        <div>
          <p className="font-[family-name:var(--font-outfit)] text-[15px] font-medium text-theme-fg">
            Need more? Top up any time.
          </p>
          <p className="mt-0.5 text-[13px] text-theme-fg/65">
            Buy credits from $10 to $1,000 — bonus up to +35% on larger packs.
            Top-up credits never expire and stack on top of your plan.
          </p>
        </div>
        <Link
          href={site.appUrl}
          className="shrink-0 rounded-lg border border-theme-fg/12 bg-theme-fg/[0.02] px-4 py-2 text-sm font-medium text-theme-fg transition-colors hover:border-theme-fg/25"
        >
          View credit packs
        </Link>
      </div>
    </div>
  );
}
