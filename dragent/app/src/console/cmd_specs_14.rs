//! Command specs shard 14: the `dragent` command itself.
//!
//! Part of the SPECS table; see cmd_suggest::specs().

use super::cmd_suggest::{CommandSpec, FlagSpec};

const GLOBAL_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--json"],
        description: "Machine-readable JSON output",
        relevance: 88.0,
    },
    FlagSpec {
        names: &["--text"],
        description: "Human-readable tables, even for agents",
        relevance: 82.0,
    },
    FlagSpec {
        names: &["--quiet"],
        description: "Print only ids and paths",
        relevance: 82.0,
    },
];

/// A command with flags but no subcommands of its own.
const fn cmd(
    name: &'static str,
    description: &'static str,
    relevance: f32,
    flags: &'static [FlagSpec],
) -> CommandSpec {
    CommandSpec {
        name,
        description,
        relevance,
        subcommands: &[],
        flags,
    }
}

const fn group(
    name: &'static str,
    description: &'static str,
    relevance: f32,
    subcommands: &'static [CommandSpec],
) -> CommandSpec {
    CommandSpec {
        name,
        description,
        relevance,
        subcommands,
        flags: &[],
    }
}

const fn flag(
    name: &'static [&'static str],
    description: &'static str,
    relevance: f32,
) -> FlagSpec {
    FlagSpec {
        names: name,
        description,
        relevance,
    }
}

const PROJECT_FLAG: FlagSpec = flag(&["--project"], "The project (PATH)", 85.0);
const TEAM_FLAG: FlagSpec = flag(
    &["--team"],
    "The team, outside its own terminals (ID)",
    70.0,
);

const ACCOUNTS_SUBS: &[CommandSpec] = &[
    cmd(
        "list",
        "Provider accounts on this machine",
        100.0,
        &[flag(&["--agent"], "Only this agent's accounts (KEY)", 85.0)],
    ),
    cmd(
        "capabilities",
        "Verified isolation support by agent",
        80.0,
        &[],
    ),
    cmd(
        "add",
        "Add an account (--agent KEY --name NAME)",
        92.0,
        &[
            flag(&["--agent"], "Agent key: claude, codex, gemini, ...", 92.0),
            flag(&["--name"], "A name for the account", 90.0),
        ],
    ),
    cmd("login", "Native sign-in on this machine (ID)", 90.0, &[]),
    cmd("status", "Check native sign-in (ID)", 88.0, &[]),
    cmd(
        "rename",
        "Rename an account (ID --name NAME)",
        80.0,
        &[flag(&["--name"], "The new name", 90.0)],
    ),
    cmd(
        "remove",
        "Remove metadata; retain native history (ID)",
        78.0,
        &[],
    ),
    cmd(
        "default",
        "The account an agent uses by default",
        84.0,
        &[
            flag(&["--agent"], "Agent key", 92.0),
            flag(&["--account"], "ID, system, or inherit", 90.0),
            PROJECT_FLAG,
        ],
    ),
];

const TEAM_SUBS: &[CommandSpec] = &[
    cmd(
        "create",
        "Start a team; an agent that runs this leads it (GOAL; alias: new)",
        100.0,
        &[
            flag(&["--agents"], "Worker agents, like claude,codex", 92.0),
            flag(&["--model"], "AGENT=MODEL, like claude=opus", 85.0),
            flag(&["--cost"], "cheapest, balanced or best", 80.0),
            flag(&["--max-parallel"], "Workers at once (N)", 78.0),
            PROJECT_FLAG,
        ],
    ),
    cmd(
        "list",
        "Teams, most recently active first",
        96.0,
        &[PROJECT_FLAG],
    ),
    cmd(
        "show",
        "The plan: tasks, their status and results ([TEAM])",
        94.0,
        &[],
    ),
    cmd("stop", "Cancel what isn't finished ([TEAM])", 88.0, &[]),
    cmd(
        "remove",
        "Take a finished or stopped team off the list ([TEAM])",
        80.0,
        &[],
    ),
    cmd(
        "guide",
        "How to lead a team (for the lead agent)",
        76.0,
        &[],
    ),
    cmd("help", "Help for teams, tasks and messages", 70.0, &[]),
];

const TASK_PROMPT_FLAGS: &[FlagSpec] = &[
    flag(&["--prompt"], "What the task is (TEXT)", 92.0),
    flag(&["--prompt-file"], "Read the prompt from FILE", 85.0),
    flag(&["--after"], "Tasks it waits for (t1,t2)", 84.0),
    flag(&["--agent"], "Agent key for the worker", 84.0),
    flag(&["--model"], "The worker's model", 80.0),
    flag(&["--review"], "Make it a review of TASK", 78.0),
    flag(
        &["--qa"],
        "The QA pass that tests the finished feature",
        76.0,
    ),
    flag(
        &["--integrated"],
        "With --qa: test the integrated checkout",
        72.0,
    ),
    flag(&["--owns"], "A file or folder it may change (repeatable)", 75.0),
    flag(&["--title"], "A new title (edit)", 74.0),
    TEAM_FLAG,
];

const TASKS_SUBS: &[CommandSpec] = &[
    cmd("list", "The plan, one task a line", 100.0, &[TEAM_FLAG]),
    cmd(
        "add",
        "Add a task (TITLE --prompt TEXT)",
        98.0,
        TASK_PROMPT_FLAGS,
    ),
    cmd("show", "One task (TASK)", 94.0, &[]),
    cmd("edit", "Change a task (TASK)", 88.0, TASK_PROMPT_FLAGS),
    cmd(
        "start",
        "Start workers on ready tasks ([TASK...])",
        94.0,
        &[],
    ),
    cmd(
        "done",
        "Finish a task ([TASK] --summary TEXT)",
        96.0,
        &[
            flag(&["--summary"], "What was done and how it was checked", 95.0),
            flag(
                &["--summary-file"],
                "Read the summary from FILE (- for stdin)",
                85.0,
            ),
            flag(&["--file"], "A file the task changed (repeatable)", 80.0),
            flag(&["--check"], "A check that was run (repeatable)", 78.0),
            flag(&["--approve"], "A review approves", 82.0),
            flag(&["--changes"], "A review asks for changes", 82.0),
            flag(
                &["--keep-uncommitted"],
                "Leave uncommitted changes out on purpose",
                70.0,
            ),
        ],
    ),
    cmd(
        "block",
        "It can't go on without something ([TASK] REASON)",
        86.0,
        &[],
    ),
    cmd(
        "wait",
        "Until a task finishes or blocks, or a message comes",
        90.0,
        &[flag(
            &["--timeout"],
            "Seconds to wait (default 100, at most 600)",
            85.0,
        )],
    ),
    cmd(
        "read",
        "A worker's terminal, as text (TASK)",
        88.0,
        &[flag(&["--lines"], "How many lines (N)", 85.0)],
    ),
    cmd(
        "keys",
        "Press keys in a worker's terminal (TASK KEY...)",
        76.0,
        &[],
    ),
    cmd(
        "diff",
        "What a task's branch changed (TASK)",
        86.0,
        &[flag(&["--patch"], "Include the diff itself", 80.0)],
    ),
    cmd(
        "integrate",
        "Bring finished work into the checkout, uncommitted (TASK...)",
        90.0,
        &[flag(&["--check"], "What you ran to verify it (repeatable)", 82.0)],
    ),
    cmd("cancel", "Cancel a task (TASK)", 80.0, &[]),
    cmd(
        "reopen",
        "Send a task back to be done again (TASK --feedback TEXT)",
        80.0,
        &[flag(&["--feedback"], "What to do differently", 92.0)],
    ),
    cmd("help", "Help for teams, tasks and messages", 70.0, &[]),
];

const ISSUES_SUBS: &[CommandSpec] = &[
    cmd(
        "list",
        "Tasks, most recently updated first",
        100.0,
        &[
            flag(&["--status"], "open, closed, all or status keys (KEYS)", 90.0),
            flag(&["--assignee"], "me, none or EMAIL", 88.0),
            flag(&["--label"], "Only tasks carrying it (L)", 84.0),
            flag(&["--project"], "here or a project KEY", 84.0),
            flag(&["--search"], "Words in key, title or description", 84.0),
        ],
    ),
    cmd("show", "The task, its comments and history (KEY)", 96.0, &[]),
    cmd(
        "create",
        "File a new task (TITLE)",
        96.0,
        &[
            flag(&["--description"], "Markdown details (TEXT)", 88.0),
            flag(&["--description-file"], "Read the description from FILE", 84.0),
            flag(&["--status"], "A status key (S)", 86.0),
            flag(&["--priority"], "urgent, high, medium, low, none (P)", 86.0),
            flag(&["--assignee"], "EMAIL or me", 86.0),
            flag(&["--label"], "A label (repeatable)", 82.0),
            flag(&["--project"], "here or a project KEY", 82.0),
        ],
    ),
    cmd(
        "update",
        "Change only what is given (KEY)",
        94.0,
        &[
            flag(&["--title"], "The new title (T)", 88.0),
            flag(&["--description"], "Markdown details (TEXT)", 84.0),
            flag(&["--description-file"], "Read the description from FILE", 82.0),
            flag(&["--status"], "A status key (S)", 86.0),
            flag(&["--priority"], "urgent, high, medium, low, none (P)", 86.0),
            flag(&["--assignee"], "EMAIL or me", 86.0),
            flag(&["--clear-assignee"], "Unassign the task", 80.0),
            flag(&["--label"], "Labels, replacing the task's (repeatable)", 82.0),
        ],
    ),
    cmd("comment", "Comment on a task (KEY TEXT)", 92.0, &[]),
    cmd(
        "link",
        "Link a pull request or tracker item (KEY --pr URL | --item PROVIDER:KEY)",
        90.0,
        &[
            flag(&["--pr"], "A pull request's address (URL)", 88.0),
            flag(&["--item"], "A tracker item (PROVIDER:KEY)", 88.0),
            flag(&["--title"], "What the link reads as (T)", 80.0),
        ],
    ),
    cmd("delete", "Delete a task, restorable for 30 days (KEY)", 84.0, &[]),
    cmd("restore", "Bring back a deleted task (KEY)", 84.0, &[]),
    cmd("help", "Help for the task list", 70.0, &[]),
];

const MSG_SUBS: &[CommandSpec] = &[
    cmd(
        "send",
        "Message lead, user, or a task's id (TO TEXT)",
        100.0,
        &[
            flag(&["--kind"], "question, answer, needs_user, ...", 82.0),
            flag(&["--reply"], "The message this answers (ID)", 80.0),
            TEAM_FLAG,
        ],
    ),
    cmd("reply", "Answer a message (ID TEXT)", 96.0, &[]),
    cmd(
        "inbox",
        "Your messages; reading marks them read",
        98.0,
        &[
            flag(&["--all"], "Read ones too", 85.0),
            flag(&["--peek"], "Don't mark them read", 82.0),
        ],
    ),
    cmd("help", "Help for teams, tasks and messages", 70.0, &[]),
];

const WORK_SUBS: &[CommandSpec] = &[
    cmd("status", "The task, and what makes it done", 100.0, &[]),
    cmd(
        "ask",
        "Ask the work item's lead; waits for the answer (QUESTION)",
        96.0,
        &[],
    ),
    cmd(
        "report",
        "Tell the lead something that affects the plan (TEXT)",
        92.0,
        &[],
    ),
    cmd(
        "message",
        "Tell another role something (ROLE TEXT)",
        88.0,
        &[],
    ),
    cmd(
        "handoff",
        "Hand another role something to take over (ROLE TEXT)",
        84.0,
        &[],
    ),
    cmd("knowledge", "What the team has recorded", 86.0, &[]),
    cmd(
        "note",
        "Record a decision, discovery or limitation (TEXT)",
        86.0,
        &[flag(&["--kind"], "The kind of note", 80.0)],
    ),
    cmd(
        "submit",
        "Finish the task (alias: done)",
        94.0,
        &[
            flag(&["--summary"], "What you changed, how you checked it", 95.0),
            flag(
                &["--summary-file"],
                "Read the summary from FILE (- for stdin)",
                85.0,
            ),
        ],
    ),
    cmd("help", "Help for the work command", 70.0, &[]),
];

const WORKSPACES_SUBS: &[CommandSpec] = &[
    CommandSpec {
        name: "list",
        description: "Workspaces in the sidebar, with branch and agents",
        relevance: 100.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["--project"],
            description: "Only workspaces of this project (PATH)",
            relevance: 85.0,
        }],
    },
    CommandSpec {
        name: "create",
        description: "A new workspace with an agent working in it (alias: new)",
        relevance: 96.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--project"],
                description: "The project (default: the one you're in, or your only one)",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["--agent"],
                description: "Agent key: claude, codex, gemini, dragent, ...",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["--prompt"],
                description: "What the agent should do (TEXT)",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--prompt-file"],
                description: "Read the prompt from FILE",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--model"],
                description: "The agent's model",
                relevance: 84.0,
            },
            FlagSpec {
                names: &["--name"],
                description: "The workspace's name in the sidebar",
                relevance: 84.0,
            },
            FlagSpec {
                names: &["--branch"],
                description: "Name for the new branch",
                relevance: 84.0,
            },
            FlagSpec {
                names: &["--base-branch"],
                description: "Branch to start from (default: the project's main branch)",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--pr"],
                description: "Start from a pull request's head (NUMBER)",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--local"],
                description: "Use the project's own checkout instead of a new worktree",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--command"],
                description: "Run a shell command in the workspace once it's open",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--tag"],
                description: "File it in a sidebar folder (repeatable)",
                relevance: 78.0,
            },
        ],
    },
    CommandSpec {
        name: "delete",
        description: "Close a workspace and remove its worktree (alias: remove)",
        relevance: 88.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["--force"],
            description: "Remove even with agents working or uncommitted changes",
            relevance: 85.0,
        }],
    },
    CommandSpec {
        name: "help",
        description: "Help for the workspaces command",
        relevance: 70.0,
        subcommands: &[],
        flags: &[],
    },
];

const PROJECTS_SUBS: &[CommandSpec] = &[
    CommandSpec {
        name: "list",
        description: "Projects open in Dragent",
        relevance: 100.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "add",
        description: "Open a folder as a project, or clone a repository (PATH | URL)",
        relevance: 96.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["--into"],
            description: "Clone into DIR (default: the current folder)",
            relevance: 85.0,
        }],
    },
];

const AGENTS_SUBS: &[CommandSpec] = &[
    CommandSpec {
        name: "list",
        description: "Agents Dragent can start on this computer",
        relevance: 100.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "running",
        description: "Agents at work, and what each is doing (alias: status)",
        relevance: 98.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["--project"],
            description: "Only agents in this project (PATH)",
            relevance: 85.0,
        }],
    },
    CommandSpec {
        name: "read",
        description: "An agent's terminal, as text (AGENT [--lines N])",
        relevance: 94.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["--lines"],
            description: "How many lines of terminal to show (N)",
            relevance: 88.0,
        }],
    },
    CommandSpec {
        name: "send",
        description: "Type a message into an agent's session (AGENT TEXT)",
        relevance: 94.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--no-enter"],
                description: "Type without pressing Enter",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--text"],
                description: "The message text",
                relevance: 82.0,
            },
        ],
    },
    CommandSpec {
        name: "stop",
        description: "End an agent's session (AGENT)",
        relevance: 90.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "start",
        description: "Another agent in an existing workspace (alias: new)",
        relevance: 92.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--workspace"],
                description: "Existing workspace (PATH)",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["--agent"],
                description: "Agent key: claude, codex, gemini, dragent, ...",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--prompt"],
                description: "What the agent should do (TEXT)",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--prompt-file"],
                description: "Read the prompt from FILE",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--model"],
                description: "The agent's model",
                relevance: 84.0,
            },
            FlagSpec {
                names: &["--from"],
                description: "Start from another agent (AGENT)",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--account"],
                description: "Provider account: ID, system, or inherit",
                relevance: 78.0,
            },
        ],
    },
    group(
        "accounts",
        "Provider accounts for agents on this machine",
        86.0,
        ACCOUNTS_SUBS,
    ),
];

const AUTOMATIONS_SUBS: &[CommandSpec] = &[
    CommandSpec {
        name: "list",
        description: "List your automations",
        relevance: 100.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "get",
        description: "Show one automation (<id>)",
        relevance: 96.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "create",
        description: "Create an automation (needs --name, --prompt, --schedule)",
        relevance: 96.0,
        subcommands: &[],
        flags: AUTOMATION_OPTIONS,
    },
    CommandSpec {
        name: "update",
        description: "Change an automation, only the options given (<id>)",
        relevance: 92.0,
        subcommands: &[],
        flags: AUTOMATION_OPTIONS,
    },
    CommandSpec {
        name: "run",
        description: "Start a run now; Dragent must be open (<id>)",
        relevance: 92.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "pause",
        description: "Stop scheduled runs (<id>)",
        relevance: 88.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "resume",
        description: "Restart scheduled runs (<id>)",
        relevance: 88.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "delete",
        description: "Delete an automation and its history (<id>)",
        relevance: 84.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "runs",
        description: "Recent runs, newest first (<id>)",
        relevance: 90.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["--limit"],
            description: "How many runs to show (N)",
            relevance: 85.0,
        }],
    },
    CommandSpec {
        name: "prompt",
        description: "Read or change the prompt (<id>)",
        relevance: 86.0,
        subcommands: &[
            CommandSpec {
                name: "get",
                description: "Print the prompt",
                relevance: 96.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "set",
                description: "Replace the prompt (--text T | --file F)",
                relevance: 94.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--text"],
                        description: "The new prompt text",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["--file"],
                        description: "Read the new prompt from FILE",
                        relevance: 88.0,
                    },
                ],
            },
            CommandSpec {
                name: "history",
                description: "Earlier prompts, newest first",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[],
    },
    CommandSpec {
        name: "copy-files",
        description: "Gitignored files copied into new worktrees (<project>)",
        relevance: 80.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["--set"],
            description: "The file list, comma-separated (P1,P2,...)",
            relevance: 85.0,
        }],
    },
    CommandSpec {
        name: "help",
        description: "Help for the automations command",
        relevance: 70.0,
        subcommands: &[],
        flags: &[],
    },
];

const AUTOMATION_OPTIONS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--name"],
        description: "The automation's name",
        relevance: 92.0,
    },
    FlagSpec {
        names: &["--prompt"],
        description: "What each run's agent should do (TEXT)",
        relevance: 90.0,
    },
    FlagSpec {
        names: &["--prompt-file"],
        description: "Read the prompt from FILE",
        relevance: 85.0,
    },
    FlagSpec {
        names: &["--schedule"],
        description: "Cron schedule, five fields: minute hour day month weekday",
        relevance: 90.0,
    },
    FlagSpec {
        names: &["--timezone"],
        description: "IANA zone (default: this computer's)",
        relevance: 80.0,
    },
    FlagSpec {
        names: &["--agent"],
        description: "Agent key: claude, codex, gemini, opencode, dragent, ...",
        relevance: 88.0,
    },
    FlagSpec {
        names: &["--project"],
        description: "Work in a project (PATH)",
        relevance: 86.0,
    },
    FlagSpec {
        names: &["--session"],
        description: "Work in a project-less folder",
        relevance: 82.0,
    },
    FlagSpec {
        names: &["--workspace"],
        description: "new: a fresh worktree each run (default) | branch | pinned",
        relevance: 84.0,
    },
    FlagSpec {
        names: &["--branch"],
        description: "The branch for --workspace branch",
        relevance: 80.0,
    },
    FlagSpec {
        names: &["--base-branch"],
        description: "Start new worktrees from B",
        relevance: 80.0,
    },
    FlagSpec {
        names: &["--push"],
        description: "Publish each new branch to the remote",
        relevance: 78.0,
    },
    FlagSpec {
        names: &["--path"],
        description: "The workspace for --workspace pinned",
        relevance: 80.0,
    },
    FlagSpec {
        names: &["--continue-session"],
        description: "With pinned: send each prompt to the previous run's agent",
        relevance: 78.0,
    },
    FlagSpec {
        names: &["--watch"],
        description: "Start runs on new items in a connected service's search",
        relevance: 82.0,
    },
    FlagSpec {
        names: &["--watch-query"],
        description: "What the watch searches for",
        relevance: 80.0,
    },
    FlagSpec {
        names: &["--check-every"],
        description: "With --watch: check every 5, 15, 30 or 60 minutes",
        relevance: 78.0,
    },
    FlagSpec {
        names: &["--no-watch"],
        description: "Run on the schedule only",
        relevance: 76.0,
    },
    FlagSpec {
        names: &["--services"],
        description: "Whether runs may write in connected services (read|write)",
        relevance: 78.0,
    },
    FlagSpec {
        names: &["--tag"],
        description: "Sidebar folder for run workspaces (repeatable)",
        relevance: 78.0,
    },
    FlagSpec {
        names: &["--no-tags"],
        description: "Don't file runs into a folder",
        relevance: 76.0,
    },
    FlagSpec {
        names: &["--paused"],
        description: "Create it switched off",
        relevance: 78.0,
    },
];

const INTEGRATIONS_SUBS: &[CommandSpec] = &[
    CommandSpec {
        name: "list",
        description: "Linear, Jira, Sentry, Slack, ...: connected or not (alias: services)",
        relevance: 100.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "describe",
        description: "Accounts, operations, schemas, scope and failure codes: [SERVICE]",
        relevance: 99.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "api",
        description: "A service's own API: SERVICE [METHOD] PATH",
        relevance: 70.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "search",
        description: "Find issues, tasks, errors, threads: SERVICE [WORDS] (alias: find)",
        relevance: 98.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "show",
        description: "Show one item (alias: read, get)",
        relevance: 94.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "comment",
        description: "Comment on an item",
        relevance: 92.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "status",
        description: "Change an item's status (alias: move)",
        relevance: 90.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "create",
        description: "Create an item (alias: new)",
        relevance: 92.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "help",
        description: "Help for the integrations command",
        relevance: 70.0,
        subcommands: &[],
        flags: &[],
    },
];

const COMPUTER_SUBS: &[CommandSpec] = &[
    cmd(
        "capabilities",
        "Discover supported desktop operations",
        99.0,
        &[],
    ),
    cmd("guide", "Desktop agent guidance for this build", 70.0, &[]),
    cmd("menus", "An app's menus, or one menu's items (APP [MENU])", 88.0, &[]),
    cmd("menu", "Choose a menu item (APP \"MENU > ITEM\")", 88.0, &[]),
    cmd(
        "window",
        "Move, resize or minimize a window (APP)",
        86.0,
        &[
            flag(&["--x"], "Left edge, screen points", 80.0),
            flag(&["--y"], "Top edge, screen points", 80.0),
            flag(&["--width"], "Width in points", 80.0),
            flag(&["--height"], "Height in points", 80.0),
            flag(&["--minimize"], "Into the Dock", 75.0),
            flag(&["--restore"], "Back from the Dock or full screen", 75.0),
            flag(&["--zoom"], "The app's zoom", 72.0),
        ],
    ),
    cmd("hotkey", "Press one modifier chord (APP CHORD)", 91.0, &[]),
    cmd(
        "paste",
        "Paste through the clipboard (APP TEXT)",
        91.0,
        &[flag(&["--text-stdin"], "Read text from stdin", 85.0)],
    ),
    cmd(
        "perform-action",
        "Run an advertised accessibility action",
        90.0,
        &[flag(&["--name"], "Exact action name", 85.0)],
    ),
    cmd(
        "drag",
        "Drag between elements or coordinates",
        90.0,
        &[
            flag(&["--from-element"], "Source element", 85.0),
            flag(&["--to-element"], "Destination element", 85.0),
        ],
    ),
    cmd(
        "permissions",
        "Check desktop accessibility, screenshots and input",
        100.0,
        &[],
    ),
    cmd("apps", "List permitted desktop apps", 98.0, &[]),
    cmd("windows", "List an app's windows (APP)", 95.0, &[]),
    cmd(
        "snapshot",
        "Read a window and capture its screenshot (APP)",
        98.0,
        &[
            flag(&["--window"], "One-based window number", 90.0),
            flag(
                &["--no-screenshot"],
                "Read only the accessibility tree",
                88.0,
            ),
        ],
    ),
    cmd(
        "click",
        "Click an element or coordinates (APP ELEMENT)",
        94.0,
        &[
            flag(&["--x"], "Window-relative x coordinate", 88.0),
            flag(&["--y"], "Window-relative y coordinate", 88.0),
            flag(&["--button"], "left, right or middle", 85.0),
            flag(&["--count"], "1, 2 or 3 clicks", 84.0),
        ],
    ),
    cmd(
        "set-value",
        "Fill an accessible field (APP ELEMENT VALUE)",
        92.0,
        &[flag(&["--stdin"], "Read the value from stdin", 85.0)],
    ),
    cmd(
        "type",
        "Type text into an app (APP TEXT)",
        92.0,
        &[flag(&["--stdin"], "Read text from stdin", 85.0)],
    ),
    cmd("key", "Press a key or chord (APP KEY)", 90.0, &[]),
    cmd(
        "scroll",
        "Scroll an app's window (APP DIRECTION)",
        90.0,
        &[
            flag(&["--element"], "Scroll this snapshot element", 85.0),
            flag(&["--amount"], "Scroll amount from 1 to 50", 85.0),
        ],
    ),
    cmd("focus", "Bring an app forward (APP)", 85.0, &[]),
    cmd("help", "Computer use command reference", 70.0, &[]),
];

pub(crate) const SPECS_14: &[CommandSpec] = &[CommandSpec {
    name: "dragent",
    description: "Dragent from the command line — workspaces, agents, automations",
    relevance: 100.0,
    subcommands: &[
        group(
            "computer",
            "Read and control desktop apps",
            85.0,
            COMPUTER_SUBS,
        ),
        CommandSpec {
            name: "workspaces",
            description: "Workspaces in the sidebar, with branch and agents (alias: workspace)",
            relevance: 100.0,
            subcommands: WORKSPACES_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "workspace",
            description: "Alias of workspaces",
            relevance: 60.0,
            subcommands: WORKSPACES_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "projects",
            description: "Projects open in Dragent (alias: project)",
            relevance: 97.0,
            subcommands: PROJECTS_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "project",
            description: "Alias of projects",
            relevance: 60.0,
            subcommands: PROJECTS_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "agents",
            description: "Agents: list, running, read, send, stop, start (alias: agent)",
            relevance: 98.0,
            subcommands: AGENTS_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "agent",
            description: "Alias of agents",
            relevance: 60.0,
            subcommands: AGENTS_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "automations",
            description: "Agents on a schedule (alias: automation)",
            relevance: 96.0,
            subcommands: AUTOMATIONS_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "automation",
            description: "Alias of automations",
            relevance: 60.0,
            subcommands: AUTOMATIONS_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "integrations",
            description: "Linear, Jira, Sentry, Slack, ... (alias: integration)",
            relevance: 94.0,
            subcommands: INTEGRATIONS_SUBS,
            flags: &[],
        },
        CommandSpec {
            name: "integration",
            description: "Alias of integrations",
            relevance: 60.0,
            subcommands: INTEGRATIONS_SUBS,
            flags: &[],
        },
        group(
            "team",
            "Agent Coordination: a lead agent and its workers",
            95.0,
            TEAM_SUBS,
        ),
        group("teams", "Alias of team", 60.0, TEAM_SUBS),
        group(
            "tasks",
            "The team's plan, and reporting on it (alias: task)",
            93.0,
            TASKS_SUBS,
        ),
        group("task", "Alias of tasks", 60.0, TASKS_SUBS),
        group(
            "issues",
            "The organisation's task list",
            92.0,
            ISSUES_SUBS,
        ),
        group("msg", "The team's mailbox (alias: message)", 91.0, MSG_SUBS),
        group("message", "Alias of msg", 60.0, MSG_SUBS),
        group(
            "work",
            "For an agent working on a work item task",
            85.0,
            WORK_SUBS,
        ),
        CommandSpec {
            name: "open",
            description: "Show a folder in Dragent (starts Dragent if needed)",
            relevance: 90.0,
            subcommands: &[],
            flags: &[],
        },
        CommandSpec {
            name: "mcp",
            description: "Dragent as an MCP server on stdio, for other agents",
            relevance: 84.0,
            subcommands: &[],
            flags: &[],
        },
        CommandSpec {
            name: "status",
            description: "Whether Dragent is running, and the organisation used",
            relevance: 88.0,
            subcommands: &[],
            flags: &[],
        },
        CommandSpec {
            name: "standup",
            description: "Read-only recap across workspaces, agents, teams and runs",
            relevance: 90.0,
            subcommands: &[CommandSpec {
                name: "setup",
                description: "Current configuration: agents, automations, services, skills",
                relevance: 86.0,
                subcommands: &[],
                flags: &[],
            }],
            flags: &[
                FlagSpec {
                    names: &["--project"],
                    description: "Only work of this project (PATH)",
                    relevance: 85.0,
                },
                FlagSpec {
                    names: &["--since"],
                    description: "Only items at or after WHEN (30m, 24h, 7d, ISO date)",
                    relevance: 84.0,
                },
                FlagSpec {
                    names: &["--limit"],
                    description: "At most N items per source (N)",
                    relevance: 84.0,
                },
                FlagSpec {
                    names: &["--setup"],
                    description: "Current configuration instead of recent work",
                    relevance: 82.0,
                },
            ],
        },
        CommandSpec {
            name: "doctor",
            description: "Layer-by-layer diagnostics: launcher, app, sign-in, agents, skills",
            relevance: 84.0,
            subcommands: &[],
            flags: &[
                FlagSpec {
                    names: &["--check"],
                    description: "Only these checks (ID,...)",
                    relevance: 85.0,
                },
                FlagSpec {
                    names: &["--project"],
                    description: "Scope workspace checks to PATH",
                    relevance: 80.0,
                },
            ],
        },
        CommandSpec {
            name: "plugins",
            description: "Packages of skills and MCP servers",
            relevance: 84.0,
            subcommands: &[
                CommandSpec {
                    name: "inspect",
                    description: "What a package brings, read without running it (SOURCE)",
                    relevance: 96.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "install",
                    description: "Install a package; its servers start switched off (SOURCE)",
                    relevance: 95.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "list",
                    description: "Installed plugins",
                    relevance: 94.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "status",
                    description: "Each step per account and server ([ID])",
                    relevance: 93.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "check",
                    description: "Call one tool to prove it works (ID --tool NAME)",
                    relevance: 88.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "update",
                    description: "A new version (ID [--from SOURCE])",
                    relevance: 87.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "disable",
                    description: "Take its skills out and its servers off (ID)",
                    relevance: 80.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "enable",
                    description: "Put its skills back (ID)",
                    relevance: 80.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "accounts",
                    description: "Add or remove an account (add|remove ID NAME)",
                    relevance: 78.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "remove",
                    description: "Remove it (ID)",
                    relevance: 82.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "sync",
                    description: "Put back missing entries and skills",
                    relevance: 70.0,
                    subcommands: &[],
                    flags: &[],
                },
            ],
            flags: &[],
        },
        CommandSpec {
            name: "setup",
            description: "Repository setup for workspaces: check, init, launch, status, stop, cleanup",
            relevance: 90.0,
            subcommands: &[
                CommandSpec {
                    name: "check",
                    description: "Validate the config and show this platform's plan ([PATH])",
                    relevance: 96.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "init",
                    description: "Copy local files and run init steps ([PATH])",
                    relevance: 96.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "launch",
                    description: "Start a launch profile (PROFILE [PATH])",
                    relevance: 94.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "status",
                    description: "Last init result and running launches ([PATH])",
                    relevance: 94.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "stop",
                    description: "Stop one launch or every launch here ([PROFILE] [PATH])",
                    relevance: 90.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "cleanup",
                    description: "Stop launches, then run cleanup steps ([PATH])",
                    relevance: 88.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "help",
                    description: "Help for the setup command",
                    relevance: 70.0,
                    subcommands: &[],
                    flags: &[],
                },
            ],
            flags: &[],
        },
        CommandSpec {
            name: "whoami",
            description: "The signed-in user and organisation",
            relevance: 86.0,
            subcommands: &[],
            flags: &[],
        },
        CommandSpec {
            name: "requests",
            description: "Whether the app finished a request a command stopped waiting for",
            relevance: 70.0,
            subcommands: &[CommandSpec {
                name: "show",
                description: "A request's state and answer, by id",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            }],
            flags: &[],
        },
        CommandSpec {
            name: "capabilities",
            description: "What this build supports: commands, tool schemas, skills, limits",
            relevance: 80.0,
            subcommands: &[],
            flags: &[],
        },
        CommandSpec {
            name: "org",
            description: "Your organisations, and switching between them",
            relevance: 86.0,
            subcommands: &[
                CommandSpec {
                    name: "list",
                    description: "Your organisations",
                    relevance: 100.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "switch",
                    description: "Switch to another organisation (alias: use)",
                    relevance: 96.0,
                    subcommands: &[],
                    flags: &[],
                },
            ],
            flags: &[],
        },
        CommandSpec {
            name: "machines",
            description: "SSH machines added to Dragent (machines list)",
            relevance: 84.0,
            subcommands: &[CommandSpec {
                name: "list",
                description: "SSH machines added to Dragent",
                relevance: 100.0,
                subcommands: &[],
                flags: &[],
            }],
            flags: &[],
        },
        CommandSpec {
            name: "settings",
            description: "Preferences: list | get KEY | set KEY VALUE | reset KEY",
            relevance: 90.0,
            subcommands: &[
                CommandSpec {
                    name: "list",
                    description: "All settable preferences",
                    relevance: 100.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "get",
                    description: "Show one preference (KEY)",
                    relevance: 96.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "set",
                    description: "Change one preference (KEY VALUE)",
                    relevance: 96.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "reset",
                    description: "Reset one preference (KEY)",
                    relevance: 88.0,
                    subcommands: &[],
                    flags: &[],
                },
            ],
            flags: &[],
        },
        CommandSpec {
            name: "theme",
            description: "Colour theme: list | set ID (alias: use)",
            relevance: 82.0,
            subcommands: &[
                CommandSpec {
                    name: "list",
                    description: "Available colour themes",
                    relevance: 100.0,
                    subcommands: &[],
                    flags: &[],
                },
                CommandSpec {
                    name: "set",
                    description: "Use a theme (ID; alias: use)",
                    relevance: 96.0,
                    subcommands: &[],
                    flags: &[],
                },
            ],
            flags: &[],
        },
        CommandSpec {
            name: "feedback",
            description: "Send feedback to the Dragent team (TEXT)",
            relevance: 78.0,
            subcommands: &[],
            flags: &[],
        },
        CommandSpec {
            name: "help",
            description: "Help for the dragent command",
            relevance: 70.0,
            subcommands: &[],
            flags: &[],
        },
        CommandSpec {
            name: "version",
            description: "The Dragent version",
            relevance: 70.0,
            subcommands: &[],
            flags: &[],
        },
    ],
    flags: GLOBAL_FLAGS,
}];
