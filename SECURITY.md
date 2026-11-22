# Security Policy

## Supported versions

Only the latest released version of Dragent receives security fixes. If you
find a vulnerability, please reproduce it on the current release before
reporting.

## How to report a vulnerability

**Do not report vulnerabilities via public GitHub issues, Discord, or
social media.** Please report privately through one of these channels:

- Preferred: GitHub private vulnerability reporting
  (repository **Security** tab → **Report a vulnerability**); or
- Email: **security@dragent.dev** — with "Security" in the
  subject line.

Please include: the affected version/commit, steps to reproduce (proof of
concept), the impact you see, and any suggested mitigation. Do not access
other users' data or degrade service availability beyond what a minimal
proof of concept requires.

## Response expectations

We aim to acknowledge your report within ~3 business days and keep you
informed as we investigate and fix. If the report is accepted, we will
coordinate disclosure with you and credit you in the release notes on
request.

## Scope

In scope: this repository's application surfaces — the desktop app
(`dragent/`, Rust), the auth portal (`auth-portal/`), and the mobile
companion (`mobile/`). Vulnerabilities in Dragent's hosted service
(accounts, sync, the relay) are in scope too: report them the same way.

Out of scope: denial-of-service/volumetric findings, social engineering,
physical-access attacks, and vulnerabilities in third-party vendors or
services Dragent depends on (report those to the vendor).
