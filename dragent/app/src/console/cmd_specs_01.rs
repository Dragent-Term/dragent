//! Command specs shard 1 (generated split; content verbatim).
//!
//! Part of the SPECS table; see cmd_suggest::specs().

use super::cmd_suggest::{CommandSpec, FlagSpec};

pub(crate) const SPECS_01: &[CommandSpec] = &[
    CommandSpec {
        name: "git",
        description: "Version control system",
        relevance: 100.0,
        subcommands: &[
            CommandSpec {
                name: "status",
                description: "Show working tree status",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-s", "--short"],
                        description: "Short format",
                        relevance: 10.0,
                    },
                    FlagSpec {
                        names: &["-b", "--branch"],
                        description: "Show branch info",
                        relevance: 8.0,
                    },
                ],
            },
            CommandSpec {
                name: "commit",
                description: "Record changes",
                relevance: 90.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-m", "--message"],
                        description: "Commit message",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["-a", "--all"],
                        description: "Stage all tracked",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--amend"],
                        description: "Amend last commit",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--no-edit"],
                        description: "Keep commit msg",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "add",
                description: "Stage file contents",
                relevance: 85.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-A", "--all"],
                        description: "Stage all changes",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["-p", "--patch"],
                        description: "Interactive staging",
                        relevance: 50.0,
                    },
                ],
            },
            CommandSpec {
                name: "push",
                description: "Push to remote",
                relevance: 80.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-u", "--set-upstream"],
                        description: "Set upstream",
                        relevance: 50.0,
                    },
                    FlagSpec {
                        names: &["-f", "--force"],
                        description: "Force push",
                        relevance: 20.0,
                    },
                    FlagSpec {
                        names: &["--force-with-lease"],
                        description: "Safer force",
                        relevance: 30.0,
                    },
                ],
            },
            CommandSpec {
                name: "pull",
                description: "Fetch and merge",
                relevance: 75.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--rebase"],
                    description: "Rebase instead of merge",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "checkout",
                description: "Switch branches / restore files",
                relevance: 70.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-b"],
                    description: "Create and switch branch",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "switch",
                description: "Switch branches",
                relevance: 68.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-c", "--create"],
                    description: "Create branch",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "branch",
                description: "List / create / delete branches",
                relevance: 65.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-d", "--delete"],
                        description: "Delete branch",
                        relevance: 60.0,
                    },
                    FlagSpec {
                        names: &["-D"],
                        description: "Force delete",
                        relevance: 50.0,
                    },
                    FlagSpec {
                        names: &["-a", "--all"],
                        description: "Show all branches",
                        relevance: 40.0,
                    },
                ],
            },
            CommandSpec {
                name: "log",
                description: "Show commit logs",
                relevance: 60.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--oneline"],
                        description: "One line per commit",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["-n"],
                        description: "Limit number",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--graph"],
                        description: "ASCII graph",
                        relevance: 50.0,
                    },
                ],
            },
            CommandSpec {
                name: "diff",
                description: "Show changes",
                relevance: 55.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--staged", "--cached"],
                    description: "Staged diff",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "stash",
                description: "Stash working directory changes",
                relevance: 50.0,
                subcommands: &[
                    CommandSpec {
                        name: "pop",
                        description: "Apply latest stash",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "push",
                        description: "Save stash",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "list",
                        description: "List stashes",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "drop",
                        description: "Delete a stash",
                        relevance: 60.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "merge",
                description: "Join branches",
                relevance: 48.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--squash"],
                        description: "Squash commits",
                        relevance: 60.0,
                    },
                    FlagSpec {
                        names: &["--no-ff"],
                        description: "No fast-forward",
                        relevance: 50.0,
                    },
                ],
            },
            CommandSpec {
                name: "rebase",
                description: "Reapply commits",
                relevance: 45.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-i", "--interactive"],
                    description: "Interactive rebase",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "reset",
                description: "Reset HEAD",
                relevance: 40.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--soft"],
                        description: "Keep changes staged",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--hard"],
                        description: "Discard all changes",
                        relevance: 60.0,
                    },
                    FlagSpec {
                        names: &["--mixed"],
                        description: "Unstage changes (default)",
                        relevance: 65.0,
                    },
                ],
            },
            CommandSpec {
                name: "clone",
                description: "Clone a repository",
                relevance: 38.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--depth"],
                    description: "Shallow clone",
                    relevance: 50.0,
                }],
            },
            CommandSpec {
                name: "fetch",
                description: "Download remote changes",
                relevance: 35.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--all"],
                        description: "Fetch all remotes",
                        relevance: 60.0,
                    },
                    FlagSpec {
                        names: &["--prune"],
                        description: "Remove deleted remotes",
                        relevance: 50.0,
                    },
                ],
            },
            CommandSpec {
                name: "tag",
                description: "Create / list tags",
                relevance: 30.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-a"],
                        description: "Annotated tag",
                        relevance: 60.0,
                    },
                    FlagSpec {
                        names: &["-d"],
                        description: "Delete tag",
                        relevance: 50.0,
                    },
                ],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["-v", "--version"],
                description: "Print version",
                relevance: 5.0,
            },
            FlagSpec {
                names: &["--help"],
                description: "Print help",
                relevance: 10.0,
            },
        ],
    },
    // ── GitHub CLI ────────────────────────────────────────────────────────────
    CommandSpec {
        name: "gh",
        description: "GitHub CLI",
        relevance: 95.0,
        subcommands: &[
            CommandSpec {
                name: "pr",
                description: "Manage pull requests",
                relevance: 100.0,
                subcommands: &[
                    CommandSpec {
                        name: "create",
                        description: "Create a PR",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[
                            FlagSpec {
                                names: &["--title", "-t"],
                                description: "PR title",
                                relevance: 90.0,
                            },
                            FlagSpec {
                                names: &["--body", "-b"],
                                description: "PR body",
                                relevance: 80.0,
                            },
                            FlagSpec {
                                names: &["--draft"],
                                description: "Draft PR",
                                relevance: 60.0,
                            },
                        ],
                    },
                    CommandSpec {
                        name: "list",
                        description: "List PRs",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "checkout",
                        description: "Checkout a PR",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "merge",
                        description: "Merge a PR",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[
                            FlagSpec {
                                names: &["--squash", "-s"],
                                description: "Squash",
                                relevance: 70.0,
                            },
                            FlagSpec {
                                names: &["--rebase", "-r"],
                                description: "Rebase",
                                relevance: 60.0,
                            },
                        ],
                    },
                    CommandSpec {
                        name: "review",
                        description: "Review a PR",
                        relevance: 60.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "repo",
                description: "Manage repositories",
                relevance: 80.0,
                subcommands: &[
                    CommandSpec {
                        name: "clone",
                        description: "Clone a repository",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "create",
                        description: "Create a repository",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "view",
                        description: "View repository info",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "issue",
                description: "Manage issues",
                relevance: 70.0,
                subcommands: &[
                    CommandSpec {
                        name: "create",
                        description: "Create an issue",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "list",
                        description: "List issues",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "close",
                        description: "Close an issue",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "auth",
                description: "Authenticate",
                relevance: 50.0,
                subcommands: &[
                    CommandSpec {
                        name: "login",
                        description: "Log in to GitHub",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "status",
                        description: "Auth status",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
        ],
        flags: &[],
    },
    // ── Rust / Cargo ──────────────────────────────────────────────────────────
    CommandSpec {
        name: "cargo",
        description: "Rust package manager",
        relevance: 100.0,
        subcommands: &[
            CommandSpec {
                name: "build",
                description: "Compile the package",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--release"],
                        description: "Release mode",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["--target"],
                        description: "Build for target",
                        relevance: 50.0,
                    },
                ],
            },
            CommandSpec {
                name: "run",
                description: "Run a binary",
                relevance: 95.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--release"],
                        description: "Release mode",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--bin"],
                        description: "Binary name",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "test",
                description: "Run tests",
                relevance: 90.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--release"],
                        description: "Release mode",
                        relevance: 50.0,
                    },
                    FlagSpec {
                        names: &["--lib"],
                        description: "Unit tests only",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "check",
                description: "Check without building",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "add",
                description: "Add dependency",
                relevance: 80.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--dev", "-D"],
                    description: "Dev dependency",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "fmt",
                description: "Format code",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "clippy",
                description: "Lint with Clippy",
                relevance: 70.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--", "-D", "warnings"],
                    description: "Deny warnings",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "update",
                description: "Update dependencies",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "clean",
                description: "Remove build artifacts",
                relevance: 55.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "doc",
                description: "Build documentation",
                relevance: 50.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--open"],
                    description: "Open in browser",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "new",
                description: "Create new package",
                relevance: 45.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--lib"],
                        description: "Library crate",
                        relevance: 60.0,
                    },
                    FlagSpec {
                        names: &["--bin"],
                        description: "Binary crate",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "publish",
                description: "Publish to crates.io",
                relevance: 30.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--dry-run"],
                    description: "Dry run",
                    relevance: 70.0,
                }],
            },
        ],
        flags: &[FlagSpec {
            names: &["-V", "--version"],
            description: "Print version",
            relevance: 5.0,
        }],
    },
    // ── Node.js — npm ─────────────────────────────────────────────────────────
    CommandSpec {
        name: "npm",
        description: "Node package manager",
        relevance: 100.0,
        subcommands: &[
            CommandSpec {
                name: "run",
                description: "Run package script",
                relevance: 100.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "install",
                description: "Install dependencies",
                relevance: 90.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-g", "--global"],
                        description: "Install globally",
                        relevance: 50.0,
                    },
                    FlagSpec {
                        names: &["-D", "--save-dev"],
                        description: "Dev dependency",
                        relevance: 80.0,
                    },
                ],
            },
            CommandSpec {
                name: "start",
                description: "Start application",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "test",
                description: "Run tests",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "build",
                description: "Build project",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "uninstall",
                description: "Remove package",
                relevance: 60.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "update",
                description: "Update packages",
                relevance: 55.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "publish",
                description: "Publish to registry",
                relevance: 40.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--dry-run"],
                    description: "Dry run",
                    relevance: 70.0,
                }],
            },
            CommandSpec {
                name: "ci",
                description: "Clean install from lockfile",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "audit",
                description: "Security audit",
                relevance: 50.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--fix"],
                    description: "Auto fix",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "outdated",
                description: "Check outdated packages",
                relevance: 45.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "list",
                description: "List installed packages",
                relevance: 40.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--depth"],
                    description: "Depth level",
                    relevance: 50.0,
                }],
            },
        ],
        flags: &[],
    },
    // ── Node.js — yarn ────────────────────────────────────────────────────────
    CommandSpec {
        name: "yarn",
        description: "Yarn package manager",
        relevance: 100.0,
        subcommands: &[
            CommandSpec {
                name: "add",
                description: "Add dependency",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-D", "--dev"],
                        description: "Dev dependency",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["-P", "--peer"],
                        description: "Peer dependency",
                        relevance: 40.0,
                    },
                    FlagSpec {
                        names: &["-O", "--optional"],
                        description: "Optional",
                        relevance: 30.0,
                    },
                ],
            },
            CommandSpec {
                name: "install",
                description: "Install all dependencies",
                relevance: 95.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--frozen-lockfile"],
                    description: "No lockfile update",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "remove",
                description: "Remove dependency",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "upgrade",
                description: "Upgrade packages",
                relevance: 70.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--latest"],
                    description: "Upgrade to latest",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "run",
                description: "Run a script",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "build",
                description: "Build project",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "test",
                description: "Run tests",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "start",
                description: "Start application",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "dev",
                description: "Start dev server",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "workspace",
                description: "Run command in workspace",
                relevance: 55.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[],
    },
    // ── Node.js — pnpm ────────────────────────────────────────────────────────
    CommandSpec {
        name: "pnpm",
        description: "pnpm package manager",
        relevance: 100.0,
        subcommands: &[
            CommandSpec {
                name: "add",
                description: "Add dependency",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-D", "--save-dev"],
                    description: "Dev dependency",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "install",
                description: "Install dependencies",
                relevance: 95.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--frozen-lockfile"],
                    description: "No lockfile update",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "remove",
                description: "Remove package",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "run",
                description: "Run script",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "build",
                description: "Build project",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "test",
                description: "Run tests",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "dev",
                description: "Start dev server",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[],
    },
    // ── Go ────────────────────────────────────────────────────────────────────
    CommandSpec {
        name: "go",
        description: "Go programming language toolchain",
        relevance: 100.0,
        subcommands: &[
            CommandSpec {
                name: "build",
                description: "Compile packages",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-o"],
                        description: "Output file name",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["-v"],
                        description: "Verbose output",
                        relevance: 50.0,
                    },
                ],
            },
            CommandSpec {
                name: "run",
                description: "Run program",
                relevance: 95.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "test",
                description: "Run tests",
                relevance: 90.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-v"],
                        description: "Verbose",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["-run"],
                        description: "Test name regex",
                        relevance: 60.0,
                    },
                    FlagSpec {
                        names: &["-cover"],
                        description: "Coverage",
                        relevance: 50.0,
                    },
                    FlagSpec {
                        names: &["-coverprofile"],
                        description: "Coverage file",
                        relevance: 40.0,
                    },
                ],
            },
            CommandSpec {
                name: "mod",
                description: "Module maintenance",
                relevance: 80.0,
                subcommands: &[
                    CommandSpec {
                        name: "init",
                        description: "Initialize module",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "tidy",
                        description: "Tidy module",
                        relevance: 95.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "vendor",
                        description: "Vendor dependencies",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "download",
                        description: "Download modules",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "get",
                description: "Add dependencies",
                relevance: 75.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-u"],
                    description: "Update packages",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "fmt",
                description: "Format source code",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "vet",
                description: "Report likely mistakes",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "install",
                description: "Compile and install",
                relevance: 60.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "generate",
                description: "Run go:generate directives",
                relevance: 50.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[FlagSpec {
            names: &["version"],
            description: "Print version",
            relevance: 5.0,
        }],
    },
    // ── Python ────────────────────────────────────────────────────────────────
    CommandSpec {
        name: "python",
        description: "Python interpreter",
        relevance: 90.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-m"],
                description: "Run module as script",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-c"],
                description: "Run command string",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-V", "--version"],
                description: "Print version",
                relevance: 5.0,
            },
        ],
    },
    CommandSpec {
        name: "python3",
        description: "Python 3 interpreter",
        relevance: 90.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-m"],
                description: "Run module as script",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-c"],
                description: "Run command string",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "pip",
        description: "Python package installer",
        relevance: 90.0,
        subcommands: &[
            CommandSpec {
                name: "install",
                description: "Install packages",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-r"],
                        description: "From requirements file",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["--upgrade", "-U"],
                        description: "Upgrade package",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["-e"],
                        description: "Editable install",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "uninstall",
                description: "Remove packages",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "list",
                description: "List installed packages",
                relevance: 70.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--outdated"],
                    description: "Show outdated",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "freeze",
                description: "Output installed packages",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "show",
                description: "Show package info",
                relevance: 50.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[],
    },
    CommandSpec {
        name: "pip3",
        description: "Python 3 package installer",
        relevance: 90.0,
        subcommands: &[
            CommandSpec {
                name: "install",
                description: "Install packages",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-r"],
                    description: "From requirements file",
                    relevance: 90.0,
                }],
            },
            CommandSpec {
                name: "uninstall",
                description: "Remove packages",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "list",
                description: "List packages",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "freeze",
                description: "Output installed packages",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[],
    },
    // ── Docker ───────────────────────────────────────────────────────────────
];
