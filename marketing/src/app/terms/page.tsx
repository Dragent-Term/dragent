import type { Metadata } from "next";
import { LegalShell, Section, Bullets } from "@/components/LegalShell";
import { site } from "@/lib/site";

export const metadata: Metadata = {
  alternates: { canonical: "/terms" },
  title: "Terms of Service",
  description: `The terms that govern your use of ${site.name}.`,
};

export default function TermsPage() {
  return (
    <LegalShell
      title="Terms of Service"
      intro={`These Terms of Service ("Terms") govern your access to and use of ${site.name} (the "Service"), operated by ${site.legalName} ("we", "us", "our"). By downloading, accessing, or using the Service, you agree to be bound by these Terms. If you do not agree, do not use the Service.`}
    >
      <Section title="1. The Service">
        <p>
          {site.name} is an AI-powered terminal application with a built-in
          coding agent. It runs commands on your device, provides AI-assisted
          diagnosis and fixes for command output, and lets you share a running
          session with people you invite. Features may change over time as we
          improve the Service.
        </p>
      </Section>

      <Section title="2. Eligibility and accounts">
        <p>
          You must be at least 18 years old, or the age of majority in your
          jurisdiction, to use the Service. You are responsible for the activity
          that occurs under your account and for keeping your login credentials
          secure. Notify us promptly of any unauthorized use.
        </p>
      </Section>

      <Section title="3. Acceptable use">
        <p>You agree not to:</p>
        <Bullets
          items={[
            "Use the Service to violate any law or infringe the rights of others;",
            "Attempt to gain unauthorized access to the Service, other accounts, or our systems;",
            "Reverse engineer, resell, or redistribute the Service except as permitted by law;",
            "Use the Service to generate or distribute malware, or to conduct attacks against third parties;",
            "Interfere with or disrupt the integrity or performance of the Service.",
          ]}
        />
        <p>
          Commands execute locally on your device and only with your approval.
          You are solely responsible for the commands you run and their effects.
        </p>
      </Section>

      <Section title="4. AI-generated output">
        <p>
          The Service uses third-party AI models to produce suggestions,
          diagnoses, and fixes. This output may be inaccurate or incomplete. You
          are responsible for reviewing any suggested command or change before
          you run or apply it. We do not warrant that AI output is correct, safe,
          or fit for any particular purpose.
        </p>
      </Section>

      <Section title="5. Plans, credits, and billing">
        <p>
          The Service offers free usage, paid subscription plans, and one-time
          credit top-ups. Prices are shown at checkout.
        </p>
        <p>
          <strong className="text-theme-fg/85">Allowances.</strong> Paid plans
          include a monthly allowance of agent usage that you can use at any
          pace — there is no per-session or per-day sub-limit. It renews each
          billing period and does not roll over. Free accounts get a monthly
          allowance on the same terms. Your remaining allowance and its reset
          date are always shown in the app and in your dashboard. Top-up credits
          are additional and never expire.
        </p>
        <p>
          <strong className="text-theme-fg/85">Credits are under your control.</strong>{" "}
          Purchased top-up credits are only spent while credit usage is switched
          on for your account. Buying credits switches it on, since that is the
          purpose of the purchase; you can switch it off at any time from the
          settings in the app, and it is off by default until you buy. While it
          is off, using up your allowance declines further requests rather than
          charging you. Your plan allowance and any free allowance are always
          used before purchased credits.
        </p>
        <p>
          Payments are processed by a third-party payment provider that acts as
          our merchant of record, which handles the transaction and any applicable
          sales tax or VAT. By purchasing, you also agree to that provider&apos;s
          terms. Subscriptions
          renew automatically at the end of each billing period until cancelled.
          You can cancel any time from your dashboard; cancellation stops future
          renewals and takes effect at the end of the current period. Refunds are
          governed by our{" "}
          <a href="/refund" className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70">
            Refund Policy
          </a>
          .
        </p>
      </Section>

      <Section title="6. Intellectual property">
        <p>
          The Service, including its software, design, and trademarks, is owned
          by {site.legalName} and protected by intellectual property laws. We
          grant you a limited, non-exclusive, non-transferable license to use the
          Service in accordance with these Terms. You retain all rights to the
          code, files, and content you create or process with the Service.
        </p>
      </Section>

      <Section title="7. Disclaimers">
        <p>
          The Service is provided &quot;as is&quot; and &quot;as available&quot;
          without warranties of any kind, whether express or implied, including
          fitness for a particular purpose and non-infringement. We do not
          warrant that the Service will be uninterrupted, error-free, or secure.
        </p>
      </Section>

      <Section title="8. Limitation of liability">
        <p>
          To the maximum extent permitted by law, {site.legalName} will not be
          liable for any indirect, incidental, special, consequential, or
          punitive damages, or for any loss of data, profits, or revenue, arising
          from your use of the Service. Our total liability for any claim will not
          exceed the amount you paid us in the twelve months before the event
          giving rise to the claim.
        </p>
      </Section>

      <Section title="9. Termination">
        <p>
          You may stop using the Service at any time. We may suspend or terminate
          your access if you breach these Terms or use the Service in a way that
          risks harm to us, other users, or third parties. On termination, your
          right to use the Service ends; sections that by their nature should
          survive will survive.
        </p>
      </Section>

      <Section title="10. Changes to these Terms">
        <p>
          We may update these Terms from time to time. If we make material
          changes, we will update the &quot;Last updated&quot; date above and,
          where appropriate, notify you. Your continued use of the Service after
          changes take effect constitutes acceptance of the revised Terms.
        </p>
      </Section>

      <Section title="11. Governing law">
        <p>
          These Terms are governed by the laws of {site.jurisdiction}, without
          regard to its conflict-of-laws rules. Any disputes will be resolved in
          the courts located there, unless applicable law requires otherwise.
        </p>
      </Section>

      <Section title="12. Contact">
        <p>
          Questions about these Terms? Reach us at{" "}
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
