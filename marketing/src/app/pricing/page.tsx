import Link from "next/link";
import { ArrowLeft } from "lucide-react";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { Pricing } from "@/components/Pricing";
import { DownloadButton } from "@/components/DownloadButton";
import { Reveal } from "@/components/Reveal";
import { site } from "@/lib/site";

export const metadata = {
  title: "Pricing",
  description:
    "Choose a plan for Dragent’s built-in agent. External CLI agent subscriptions and usage are billed by their providers.",
  alternates: { canonical: "/pricing" },
};

export default function PricingPage() {
  return (
    <>
      <Nav />
      <main id="main-content" className="relative z-10 flex-1">
        <section className="relative scroll-mt-20">
          <div className="mx-auto max-w-6xl px-5 pt-12 pb-20 sm:px-8 sm:pt-16 lg:pt-20 lg:pb-28">
            <Link
              href="/"
              className="inline-flex items-center gap-1.5 text-sm text-theme-fg/50 transition-colors hover:text-theme-fg"
            >
              <ArrowLeft size={15} /> Back home
            </Link>

            <Reveal className="mx-auto mt-12 max-w-2xl text-center">
              <p className="font-[family-name:var(--font-mono-jb)] text-xs uppercase tracking-[0.2em] ribbon-text">
                Pricing
              </p>
              <h1 className="mt-3 font-[family-name:var(--font-outfit)] text-4xl font-semibold tracking-tight text-theme-fg sm:text-5xl">
                Your workspace. Your pace.
              </h1>
              <p className="mt-4 text-[15px] leading-relaxed text-theme-fg/55">
                Plans cover usage of the built-in Dragent agent. Paid plans
                share model access and differ in included monthly usage.
                Installed CLI agents use their own provider accounts and
                billing.
              </p>
            </Reveal>

            <div className="mt-14">
              <Pricing />
            </div>
          </div>
        </section>

        <div className="pricing-billing-note container-wide">
          <h2>What about my existing agent subscription?</h2>
          <p>
            Claude Code, Codex, and other external CLIs use their own provider
            login and billing. A Dragent plan does not include those
            subscriptions. Check your account dashboard for the current built-in
            agent allowance, credit balance, and billing terms before choosing a
            plan.
          </p>
          <Link href="/docs/billing">Understand usage and billing →</Link>
        </div>

        <section className="relative overflow-hidden border-t border-theme-fg/8">
          <div className="relative mx-auto max-w-6xl px-5 py-24 sm:px-8">
            <Reveal className="max-w-2xl">
              <h2 className="font-[family-name:var(--font-outfit)] text-4xl font-semibold leading-tight tracking-tight sm:text-5xl">
                <span className="text-theme-fg">Bring your agents</span>
                <br />
                <span className="text-theme-fg">into the picture.</span>
              </h2>
              <p className="mt-5 max-w-lg text-[16px] leading-relaxed text-theme-fg/55">
                Start a focused workspace, run your preferred agent, and keep
                the changes and review close to the task.
              </p>
              <div className="mt-9 flex flex-col items-start gap-3 sm:flex-row">
                <DownloadButton size="lg" />
                <Link
                  href={site.appUrl}
                  className="inline-flex items-center justify-center gap-1.5 rounded-lg border border-theme-fg/15 bg-theme-fg/[0.04] px-7 py-3.5 text-[15px] font-medium text-theme-fg transition-colors hover:border-theme-fg/30 hover:bg-theme-fg/[0.07]"
                >
                  Open dashboard
                </Link>
              </div>
            </Reveal>
          </div>
        </section>
      </main>
      <Footer />
    </>
  );
}
