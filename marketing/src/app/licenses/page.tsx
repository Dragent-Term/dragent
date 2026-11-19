import type { Metadata } from "next";
import { LegalShell, Section, Bullets } from "@/components/LegalShell";
import { site } from "@/lib/site";
import { dependencyLicenses } from "@/lib/license-texts";

export const metadata: Metadata = {
  alternates: { canonical: "/licenses" },
  title: "Open-Source Licenses & Notices",
  description: `Third-party open-source software and license notices for ${site.name}.`,
};

export default function LicensesPage() {
  return (
    <LegalShell
      title="Open-Source Licenses & Notices"
      intro={`${site.name}'s own code is open-source software licensed under the GNU Affero General Public License, version 3 only (AGPL-3.0-only), including for commercial use under its copyleft terms. If you convey the software, or run a modified version over a network for others, you must offer those users the Corresponding Source. Like most open-source software, it incorporates certain third-party open-source components that remain licensed under their own terms. ${site.name} complies with those licenses' attribution and notice requirements. The notices below identify each component, and the complete, verbatim text of every license follows at the end of this page. The same notices are distributed with the application. These third-party terms apply only to the respective third-party components, not to ${site.name}'s own code, which is licensed under AGPL-3.0-only.`}
    >
      <Section title="Application framework — WarpUI (MIT)">
        <p>
          {site.name}&apos;s desktop application is built on <strong>WarpUI</strong>,
          the MIT-licensed UI framework from Warp, comprising the{" "}
          <code>warpui</code> and <code>warpui_core</code> crates.
        </p>
        <Bullets
          items={[
            "warpui — MIT License, © Denver Technologies, Inc. (Warp).",
            "warpui_core — MIT License, © Denver Technologies, Inc. (Warp).",
            "mermaid-to-svg — MIT License, © Denver Technologies, Inc. (Warp). Pure-Rust Mermaid diagram renderer; includes code derived from mermaid.js and dagre.js.",
          ]}
        />
        <p>
          The MIT license text and copyright notice for WarpUI are retained and
          distributed with the application.
        </p>
      </Section>

      <Section title="Components under the Apache License 2.0">
        <p>
          The following components are used under the Apache License 2.0. For each,{" "}
          {site.name} retains a copy of the License, preserves the required
          copyright and attribution notices, marks any files it modified with a
          notice of change, and reproduces the attribution in the notices
          distributed with the product.
        </p>
        <Bullets
          items={[
            "Alacritty terminal library — © The Alacritty Project. Portions of the terminal core are used under Apache-2.0; modified source files carry a notice of the change.",
            "Warp Themes (github.com/warpdotdev/themes) — Apache-2.0. Bundled theme catalog; some theme files have been modified, as noted in the product notices.",
            "Warp Workflows / commands.dev (github.com/warpdotdev/Workflows) — Apache-2.0. Bundled workflow catalog.",
          ]}
        />
        <p>
          The complete Apache License, Version 2.0 is reproduced{" "}
          <a
            href="#alacritty"
            className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
          >
            in full below
          </a>
          , is included with the application, and is available at{" "}
          <a
            href="https://www.apache.org/licenses/LICENSE-2.0"
            className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
            target="_blank"
            rel="noreferrer noopener"
          >
            apache.org/licenses/LICENSE-2.0
          </a>
          .
        </p>
      </Section>

      <Section title="Fonts">
        <p>
          <strong>Fira Code</strong> — SIL Open Font License 1.1, © 2014 The Fira
          Code Project Authors. The OFL license and copyright notice are
          distributed with the font files. &quot;Fira Code&quot; is a Reserved
          Font Name.
        </p>
      </Section>

      <Section title="Other third-party components">
        <Bullets
          items={[
            "common-skills — MIT License, © 2026 Denver Technologies, Inc. Bundled agent skills (some removed or adapted).",
            "Lucide icons — ISC License (portions MIT, Feather-derived), © Lucide Contributors.",
            "Microsoft Windows Terminal assets/shaders — MIT License, © Microsoft Corporation.",
            "DirectX Shader Compiler / LLVM — University of Illinois/NCSA Open Source License.",
            "Rust crate dependencies — used under their respective open-source licenses, predominantly MIT and/or Apache-2.0.",
          ]}
        />
        <p>
          {site.name}&apos;s own code is copyleft under AGPL-3.0-only; no other
          copyleft (GPL / LGPL / MPL) code beyond the licenses above is included.
        </p>
      </Section>

      <Section title="Trademarks">
        <p>
          Product names and logos shown within the application — including Claude
          (Anthropic, PBC), ChatGPT / OpenAI (OpenAI, L.L.C.), and Ollama — are
          the trademarks of their respective owners, used nominatively only to
          identify those products. Their inclusion does not imply endorsement or
          affiliation. {site.name} and its brand marks are the property of{" "}
          {site.legalName}.
        </p>
      </Section>

      <Section title="Full license text">
        <p>
          The complete, unmodified licence text for every third-party component
          above, reproduced verbatim from the project&apos;s own LICENSE file —
          original wording, line breaks, and indentation preserved, nothing
          abridged. Components sharing a licence each carry their own full copy
          rather than a cross-reference, so every notice stands on its own.
        </p>
        {dependencyLicenses.map((d) => (
          <div key={d.id} id={d.id} className="mt-10 scroll-mt-24">
            <h3 className="font-[family-name:var(--font-outfit)] text-lg font-semibold text-theme-fg">
              {d.license} — {d.dep}
            </h3>
            <div className="mt-1 h-px w-full bg-theme-fg/10" />
            <p className="mt-2 text-[13px] leading-relaxed text-theme-fg/45">
              {d.copyright} · Source: {d.source}
            </p>
            <pre className="mt-3 max-h-[28rem] overflow-auto rounded-xl border border-theme-fg/8 bg-surface-raised p-4 font-[family-name:var(--font-mono-jb)] text-[11.5px] leading-[1.6] whitespace-pre text-theme-fg/60">
              {d.text}
            </pre>
          </div>
        ))}
      </Section>

      <Section title="Questions">
        <p>
          If you need any of these notices in another format, contact us at{" "}
          <a
            href={`mailto:${site.contactEmail}`}
            className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
          >
            {site.contactEmail}
          </a>
          .
        </p>
      </Section>
    </LegalShell>
  );
}
