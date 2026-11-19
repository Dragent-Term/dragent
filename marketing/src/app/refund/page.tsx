import type { Metadata } from "next";
import { LegalShell, Section, Bullets } from "@/components/LegalShell";
import { site } from "@/lib/site";

export const metadata: Metadata = {
  alternates: { canonical: "/refund" },
  title: "Refund Policy",
  description: `${site.name}'s refund policy for subscriptions and credit top-ups.`,
};

export default function RefundPage() {
  return (
    <LegalShell
      title="Refund Policy"
      intro={`We want you to be happy with ${site.name}. This policy explains when you can get a refund. Payments are processed by a third-party payment provider that acts as our merchant of record, and refunds are issued through them back to your original payment method.`}
    >
      <Section title="Subscriptions">
        <p>
          If you are not satisfied with a paid subscription, you may request a
          full refund within <strong>14 days</strong> of the initial purchase or a
          renewal charge, provided you have used only a minimal amount of the
          plan&apos;s monthly agent allowance. Because agent usage has a real,
          per-request cost that we pay to model providers, refunds may be declined
          or prorated where a significant portion of the allowance has already
          been consumed.
        </p>
        <p>
          You can cancel a subscription at any time from your dashboard.
          Cancellation stops future renewals; you keep access until the end of the
          current billing period. We do not automatically refund partial periods
          on cancellation.
        </p>
      </Section>

      <Section title="Credit top-ups">
        <p>
          One-time credit top-ups are refundable within{" "}
          <strong>14 days</strong> of purchase <em>only if the credits are
          entirely unused</em>. Once any portion of a top-up has been spent, that
          top-up is non-refundable, because the credits reflect usage costs we
          incur on your behalf. Purchased credits do not expire.
        </p>
      </Section>

      <Section title="When refunds are not available">
        <Bullets
          items={[
            "Subscription allowance or top-up credits that have been substantially or fully used;",
            "Requests made more than 14 days after the charge;",
            "Accounts terminated for breach of our Terms of Service;",
            "Charges disputed through a chargeback without first contacting us (please reach out — we resolve most issues quickly).",
          ]}
        />
      </Section>

      <Section title="How to request a refund">
        <p>
          Email{" "}
          <a
            href={`mailto:${site.contactEmail}`}
            className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
          >
            {site.contactEmail}
          </a>{" "}
          from the address on your account, with your order number and a short
          note about what went wrong. We aim to respond within 2 business days.
          Approved refunds are processed by our payment provider to your original
          payment method; the time for funds to appear depends on your bank or
          card issuer.
        </p>
      </Section>

      <Section title="Exceptions and fairness">
        <p>
          We handle refund requests in good faith and may make exceptions to this
          policy at our discretion — for example, in cases of a genuine billing
          error or a service outage that prevented you from using what you paid
          for. This policy does not limit any rights you have under applicable
          consumer-protection law.
        </p>
      </Section>
    </LegalShell>
  );
}
