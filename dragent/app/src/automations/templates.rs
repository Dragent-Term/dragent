//! Starting points for new automations. Picking one fills the form; the
//! user adjusts project, agent, and schedule before creating it.

use crate::icons::AppIcon;

pub struct Template {
    pub name: &'static str,
    pub summary: &'static str,
    pub category: &'static str,
    pub icon: AppIcon,
    pub schedule: &'static str,
    pub prompt: &'static str,
}

pub const TEMPLATES: [Template; 6] = [
    Template {
        name: "Hunt for serious bugs",
        summary: "Go through the latest commits looking for real correctness problems and fix the ones that matter",
        category: "Code health",
        icon: AppIcon::Asset("icons/bug.svg"),
        schedule: "0 10 * * 1",
        prompt: "Go through the commits from the past week and look for bugs that would actually hurt users: wrong results, crashes, races, lost data, and edge cases the code now gets wrong. For each real problem, make the smallest change that fixes it and prove it with a focused test. Leave style nits and speculative concerns out; finish with a short list of what you found and changed.",
    },
    Template {
        name: "Daily change digest",
        summary: "Each morning, write up what changed in the repository yesterday and what deserves attention",
        category: "Reporting",
        icon: AppIcon::Asset("icons/mail.svg"),
        schedule: "0 9 * * *",
        prompt: "Write a brief digest of what changed in this repository over the last day. Group it into what shipped, anything risky (migrations, config, dependency or security-sensitive changes), work that looks stuck or half-finished, and suggested next steps. Keep it skimmable: short bullets, linked to commits or files.",
    },
    Template {
        name: "Security sweep",
        summary: "Review the codebase on a schedule for exploitable weaknesses and report the confirmed ones",
        category: "Security",
        icon: AppIcon::Lock,
        schedule: "0 11 * * 1",
        prompt: "Review the codebase for security weaknesses an attacker could actually use. Pay particular attention to authentication and authorization checks, injection into queries or shell commands, unsafe file or path handling, secrets in code or logs, outbound requests built from user input, and untrusted deserialization. Only report issues you can explain an exploit path for, rank them by impact, and fix the clear-cut ones.",
    },
    Template {
        name: "Strengthen test coverage",
        summary: "Find risky recent logic with thin tests and add focused tests for it",
        category: "Code health",
        icon: AppIcon::TaskList,
        schedule: "0 10 * * 2",
        prompt: "Look at recently changed code and find the logic most likely to break without anyone noticing: new branches, bug fixes that shipped without a test, parsing and validation, permission checks, shared helpers, and concurrency. Add small, deterministic tests for that behaviour using the project's existing test style; skip snapshot churn and trivial getters. Run the affected tests and summarise which risks are now covered.",
    },
    Template {
        name: "Work through reported bugs",
        summary: "Pick up bug reports from issues or notes, reproduce them, and prepare fixes",
        category: "Triage",
        icon: AppIcon::Tool,
        schedule: "0 10 * * 1",
        prompt: "Find the open bug reports this project tracks (linked issues, a bugs doc, or notes in the repository). For each one with enough detail, reproduce it or trace it to the code responsible, make a minimal fix, and add a test that would have caught it. Prepare a pull-request style summary per fix. If a report lacks the detail needed, list exactly what's missing instead of guessing.",
    },
    Template {
        name: "Refresh developer docs",
        summary: "Bring documentation up to date for code that changed or was never documented",
        category: "Documentation",
        icon: AppIcon::Asset("icons/book-open.svg"),
        schedule: "0 14 * * 5",
        prompt: "Find developer-facing code that changed recently or has no documentation, and update the docs to match: what it does, how to set it up, a short example, known pitfalls, and how to check it works. Edit existing pages rather than adding near-duplicates, and keep the tone consistent with the rest of the docs.",
    },
];
