import Link from "next/link";
import { ArrowLeft } from "lucide-react";
import { Nav } from "./Nav";
import { Footer } from "./Footer";
import { site } from "@/lib/site";

export function LegalShell({
  title,
  intro,
  children,
}: {
  title: string;
  intro?: string;
  children: React.ReactNode;
}) {
  return (
    <>
      <Nav />
      <main id="main-content" className="relative z-10 flex-1">
        <div className="mx-auto max-w-3xl px-5 py-16 sm:px-8 lg:py-20">
          <Link
            href="/"
            className="inline-flex items-center gap-1.5 text-sm text-theme-fg/50 transition-colors hover:text-theme-fg"
          >
            <ArrowLeft size={15} /> Back home
          </Link>

          <h1 className="mt-8 font-[family-name:var(--font-outfit)] text-4xl font-semibold tracking-tight text-theme-fg">
            {title}
          </h1>
          <p className="mt-3 font-[family-name:var(--font-mono-jb)] text-xs text-theme-fg/40">
            Last updated {site.lastUpdated}
          </p>
          {intro && (
            <p className="mt-6 text-[15px] leading-relaxed text-theme-fg/60">
              {intro}
            </p>
          )}

          <article className="legal mt-10">{children}</article>

          <div className="mt-14 rounded-2xl border border-theme-fg/8 bg-theme-fg/[0.02] p-5 text-[13px] leading-relaxed text-theme-fg/50">
            Questions about this policy? Email{" "}
            <a
              href={`mailto:${site.contactEmail}`}
              className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
            >
              {site.contactEmail}
            </a>
            .
          </div>
        </div>
      </main>
      <Footer />
    </>
  );
}

/** Section heading + body helpers so the three legal pages read consistently. */
export function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="mt-9 first:mt-0">
      <h2 className="font-[family-name:var(--font-outfit)] text-xl font-semibold text-theme-fg">
        {title}
      </h2>
      <div className="mt-3 space-y-3 text-[14.5px] leading-relaxed text-theme-fg/65">
        {children}
      </div>
    </section>
  );
}

export function Bullets({ items }: { items: string[] }) {
  return (
    <ul className="space-y-2">
      {items.map((it, i) => (
        <li key={i} className="flex gap-2.5">
          <span className="mt-2 h-1 w-1 shrink-0 rounded-full bg-theme-fg/60" />
          <span>{it}</span>
        </li>
      ))}
    </ul>
  );
}
