/**
 * Central site config. URLs are env-overridable so the same build can point at
 * staging or prod. The account portal retains its existing beta default — set NEXT_PUBLIC_* vars
 * in Amplify (or .env.local) before go-live.
 */
export const site = {
  name: "Dragent",
  tagline: "Run an agent team, not five terminals",
  description:
    "Dragent is an agentic development environment where a lead agent splits your goal across Claude Code, Codex, and the agents you already use, each in its own Git worktree. They message each other, a checker tests the work, and you're asked only when a decision is yours.",
  // Where "Sign in" / "Dashboard" go — the auth-portal deployment.
  appUrl: process.env.NEXT_PUBLIC_APP_URL ?? "https://beta.dragent.dev",
  docsUrl: process.env.NEXT_PUBLIC_DOCS_URL ?? "/docs",
  // Release assets, per OS and architecture. `null` = not shipping yet.
  releaseBase: "https://d8d93sixrrr9g.cloudfront.net/stable/latest",
  downloads: {
    mac: {
      arm64:
        "https://d8d93sixrrr9g.cloudfront.net/stable/latest/Dragent-macos-arm64.dmg",
      x64: "https://d8d93sixrrr9g.cloudfront.net/stable/latest/Dragent-macos-x86_64.dmg",
    },
    windows: {
      arm64:
        "https://d8d93sixrrr9g.cloudfront.net/stable/latest/Dragent-windows-arm64-setup.exe",
      x64: "https://d8d93sixrrr9g.cloudfront.net/stable/latest/Dragent-windows-x64-setup.exe",
    },
    linux: null,
  },
  // Primary build the hero button links to, per OS (most common arch).
  primary: {
    mac: "https://d8d93sixrrr9g.cloudfront.net/stable/latest/Dragent-macos-arm64.dmg",
    windows:
      "https://d8d93sixrrr9g.cloudfront.net/stable/latest/Dragent-windows-x64-setup.exe",
  },
  // Legal / contact.
  legalName: "Dragent",
  contactEmail: "support@dragent.dev",
  // Physical/mailing line shown in legal pages — replace with your real one.
  jurisdiction: "the United States",
  lastUpdated: "July 21, 2026",
} as const;

export const nav = [
  { label: "Features", href: "/#features" },
  { label: "Coordination", href: "/#coordination" },
  { label: "Pricing", href: "/pricing" },
  { label: "Docs", href: site.docsUrl },
];
