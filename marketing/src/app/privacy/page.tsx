import type { Metadata } from "next";
import { LegalShell, Section, Bullets } from "@/components/LegalShell";
import { site } from "@/lib/site";

export const metadata: Metadata = {
  alternates: { canonical: "/privacy" },
  title: "Privacy Policy",
  description: `How ${site.name} collects, uses, and protects your data.`,
};

export default function PrivacyPage() {
  return (
    <LegalShell
      title="Privacy Policy"
      intro={`This Privacy Policy explains how ${site.legalName} ("we", "us", "our") collects, uses, and protects your information when you use ${site.name} (the "Service"). We aim to collect as little as we can and to be clear about what we do with it.`}
    >
      <Section title="Information we collect">
        <p>We collect the following categories of information:</p>
        <Bullets
          items={[
            "Account information — your name and email address when you create an account.",
            "Usage and billing data — your plan, credit balance, and metered agent usage, so we can meter and bill accurately.",
            "AI interaction data — the command output and context you send to the agent so it can produce a diagnosis or fix.",
            "Shared session data — when you share a session, the terminal output and input relayed to the people you invite, kept only for the life of the session.",
            "Technical data — basic device and diagnostic information needed to operate and secure the Service.",
          ]}
        />
      </Section>

      <Section title="What we do not do">
        <Bullets
          items={[
            "We do not sell your personal data.",
            "We do not read or execute commands on your device; commands run locally and only with your approval.",
            "We do not use your private code or command output to train our own models.",
          ]}
        />
      </Section>

      <Section title="How we use your information">
        <p>We use your information to:</p>
        <Bullets
          items={[
            "Provide, maintain, and improve the Service;",
            "Process the agent requests you make and return results;",
            "Meter usage, process payments, and prevent fraud or abuse;",
            "Communicate with you about your account, security, and updates;",
            "Comply with legal obligations.",
          ]}
        />
      </Section>

      <Section title="AI model providers">
        <p>
          To generate diagnoses and fixes, we send the relevant context to
          third-party AI providers (which may include Anthropic, OpenAI, and
          xAI). These providers process the data to return a result and, under
          their business terms, do not use it to train their models. We send only
          what is needed to fulfill your request.
        </p>
      </Section>

      <Section title="Payments">
        <p>
          Payments are handled by a third-party payment provider that acts as our
          merchant of record. When you make a purchase, your payment details are
          collected and processed by that provider under its own privacy policy —
          we do not receive or store your full card details. We receive
          confirmation of the transaction and the plan or credits you purchased.
        </p>
      </Section>

      <Section title="Data sharing">
        <p>
          We share information only with service providers who help us operate the
          Service (such as hosting, payment processing, and AI model providers),
          bound by confidentiality and data-protection obligations, and where
          required by law or to protect our rights and users.
        </p>
      </Section>

      <Section title="Data retention and security">
        <p>
          We keep your information for as long as your account is active or as
          needed to provide the Service and meet legal obligations, then delete or
          anonymize it. We use encryption in transit and at rest, along with
          access controls and other safeguards. No system is
          perfectly secure, but we work to protect your data.
        </p>
      </Section>

      <Section title="Your rights">
        <p>
          Depending on where you live, you may have the right to access, correct,
          export, or delete your personal data, and to object to or restrict
          certain processing. To exercise any of these, email us at{" "}
          <a
            href={`mailto:${site.contactEmail}`}
            className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
          >
            {site.contactEmail}
          </a>
          . You can also delete your account at any time from your dashboard.
        </p>
      </Section>

      <Section title="International transfers">
        <p>
          We may process and store information in countries other than your own.
          Where we transfer data internationally, we take steps to ensure it
          receives an appropriate level of protection consistent with this
          Policy.
        </p>
      </Section>

      <Section title="Children">
        <p>
          The Service is not directed to children under 18, and we do not
          knowingly collect their personal data. If you believe a child has
          provided us information, contact us and we will delete it.
        </p>
      </Section>

      <Section title="Changes to this Policy">
        <p>
          We may update this Policy from time to time. We will revise the
          &quot;Last updated&quot; date above and, for material changes, take
          reasonable steps to notify you.
        </p>
      </Section>

      <Section title="Contact">
        <p>
          For any privacy question or request, email{" "}
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
