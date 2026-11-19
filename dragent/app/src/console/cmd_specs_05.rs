//! Command specs shard 5 (generated split; content verbatim).
//!
//! Part of the SPECS table; see cmd_suggest::specs().

use super::cmd_suggest::{CommandSpec, FlagSpec};

pub(crate) const SPECS_05: &[CommandSpec] = &[
    CommandSpec {
        name: "sst",
        description: "SST — Serverless Stack framework",
        relevance: 88.0,
        subcommands: &[
            CommandSpec {
                name: "dev",
                description: "Start SST dev mode (Live Lambda)",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--stage"],
                        description: "Stage name",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--region"],
                        description: "AWS region",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--profile"],
                        description: "AWS profile",
                        relevance: 65.0,
                    },
                ],
            },
            CommandSpec {
                name: "deploy",
                description: "Deploy app to AWS",
                relevance: 95.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--stage"],
                        description: "Stage name",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--region"],
                        description: "AWS region",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--profile"],
                        description: "AWS profile",
                        relevance: 65.0,
                    },
                ],
            },
            CommandSpec {
                name: "remove",
                description: "Remove deployed app",
                relevance: 80.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--stage"],
                    description: "Stage name",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "build",
                description: "Build app for deployment",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--stage"],
                    description: "Stage name",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "secrets",
                description: "Manage app secrets",
                relevance: 75.0,
                subcommands: &[
                    CommandSpec {
                        name: "set",
                        description: "Set a secret",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "list",
                        description: "List secrets",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "remove",
                        description: "Remove a secret",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "load",
                        description: "Load secrets from .env",
                        relevance: 60.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "console",
                description: "Open SST console in browser",
                relevance: 70.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--stage"],
                    description: "Stage name",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "diff",
                description: "Compare with deployed state",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "bind",
                description: "Run process with resource bindings",
                relevance: 60.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "create",
                description: "Create a new SST project",
                relevance: 55.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--template"],
                    description: "Project template",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "update",
                description: "Update SST to latest version",
                relevance: 50.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--stage"],
                description: "Stage name (dev/prod)",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--region"],
                description: "AWS region",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--profile"],
                description: "AWS CLI profile",
                relevance: 65.0,
            },
        ],
    },
    // ── Linux / Unix core commands ────────────────────────────────────────────
    CommandSpec {
        name: "ls",
        description: "List directory contents",
        relevance: 95.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-la", "-al"],
                description: "Long listing with hidden files",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-l"],
                description: "Long listing format",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-a", "--all"],
                description: "Show hidden files",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-h", "--human-readable"],
                description: "Human readable sizes",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-R", "--recursive"],
                description: "List subdirectories",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["-t"],
                description: "Sort by modification time",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-S"],
                description: "Sort by file size",
                relevance: 55.0,
            },
            FlagSpec {
                names: &["--color"],
                description: "Colorized output",
                relevance: 50.0,
            },
        ],
    },
    CommandSpec {
        name: "cd",
        description: "Change directory",
        relevance: 95.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "mkdir",
        description: "Create directories",
        relevance: 88.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-p", "--parents"],
                description: "Create parent directories",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Print each created dir",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-m", "--mode"],
                description: "Set permissions",
                relevance: 55.0,
            },
        ],
    },
    CommandSpec {
        name: "rm",
        description: "Remove files or directories",
        relevance: 85.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-rf"],
                description: "Recursive force remove",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-r", "--recursive"],
                description: "Remove recursively",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-f", "--force"],
                description: "Force remove",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-i"],
                description: "Prompt before each removal",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Explain what is done",
                relevance: 55.0,
            },
        ],
    },
    CommandSpec {
        name: "cp",
        description: "Copy files or directories",
        relevance: 85.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-r", "--recursive"],
                description: "Copy recursively",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-p", "--preserve"],
                description: "Preserve attributes",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Show what is copied",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["-f", "--force"],
                description: "Force overwrite",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-n", "--no-clobber"],
                description: "No overwrite",
                relevance: 55.0,
            },
        ],
    },
    CommandSpec {
        name: "mv",
        description: "Move or rename files",
        relevance: 85.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-f", "--force"],
                description: "Force overwrite",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-i", "--interactive"],
                description: "Prompt before overwrite",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Show what is moved",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["-n", "--no-clobber"],
                description: "No overwrite",
                relevance: 55.0,
            },
        ],
    },
    CommandSpec {
        name: "cat",
        description: "Concatenate and print files",
        relevance: 80.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-n", "--number"],
                description: "Number all lines",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-A", "--show-all"],
                description: "Show all characters",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-s", "--squeeze-blank"],
                description: "Squeeze blank lines",
                relevance: 55.0,
            },
        ],
    },
    CommandSpec {
        name: "chmod",
        description: "Change file permissions",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-R", "--recursive"],
                description: "Change recursively",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Verbose output",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-c", "--changes"],
                description: "Report only changes",
                relevance: 55.0,
            },
        ],
    },
    CommandSpec {
        name: "chown",
        description: "Change file owner and group",
        relevance: 72.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-R", "--recursive"],
                description: "Change recursively",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Verbose output",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "tar",
        description: "Archive files",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-czf"],
                description: "Create gzipped archive",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-xzf"],
                description: "Extract gzipped archive",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-cjf"],
                description: "Create bzip2 archive",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-xf"],
                description: "Extract archive",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-tvf"],
                description: "List archive contents",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-c"],
                description: "Create archive",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-x"],
                description: "Extract archive",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-z"],
                description: "Gzip compression",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["-j"],
                description: "bzip2 compression",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-v"],
                description: "Verbose output",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-f"],
                description: "Archive file name",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--strip-components"],
                description: "Strip N leading components",
                relevance: 50.0,
            },
        ],
    },
    CommandSpec {
        name: "ps",
        description: "Report process status",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["aux"],
                description: "All processes (BSD style)",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-ef"],
                description: "All processes (POSIX style)",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-a"],
                description: "All users' processes",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-u"],
                description: "User-oriented output",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["-x"],
                description: "Without controlling terminal",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "kill",
        description: "Send signal to processes",
        relevance: 72.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-9", "-KILL"],
                description: "Force kill (SIGKILL)",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-15", "-TERM"],
                description: "Terminate (SIGTERM)",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-l"],
                description: "List all signals",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "killall",
        description: "Kill processes by name",
        relevance: 68.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-9"],
                description: "Force kill",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-v"],
                description: "Verbose",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "head",
        description: "Output first lines of file",
        relevance: 72.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["-n", "--lines"],
            description: "Number of lines",
            relevance: 90.0,
        }],
    },
    CommandSpec {
        name: "tail",
        description: "Output last lines of file",
        relevance: 72.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-f", "--follow"],
                description: "Follow appended data",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-n", "--lines"],
                description: "Number of lines",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-F"],
                description: "Follow by name (retry)",
                relevance: 75.0,
            },
        ],
    },
    CommandSpec {
        name: "sort",
        description: "Sort lines of text",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-r", "--reverse"],
                description: "Reverse sort order",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-n", "--numeric-sort"],
                description: "Numeric sort",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-u", "--unique"],
                description: "Remove duplicates",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-k"],
                description: "Sort by key (column)",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-t"],
                description: "Field separator",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "wc",
        description: "Word, line, byte count",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-l", "--lines"],
                description: "Count lines",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-w", "--words"],
                description: "Count words",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-c", "--bytes"],
                description: "Count bytes",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "sed",
        description: "Stream editor",
        relevance: 68.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-i"],
                description: "In-place edit",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-e"],
                description: "Script expression",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-n"],
                description: "Suppress auto-print",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-r", "-E"],
                description: "Extended regex",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "awk",
        description: "Pattern scanning and processing",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-F"],
                description: "Field separator",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-v"],
                description: "Set variable",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-f"],
                description: "Read program from file",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "env",
        description: "Print or set environment",
        relevance: 68.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-i"],
                description: "Start with empty environment",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-u"],
                description: "Remove variable from environment",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "export",
        description: "Set environment variable",
        relevance: 75.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "source",
        description: "Execute commands in current shell",
        relevance: 72.0,
        subcommands: &[],
        flags: &[],
    },
    CommandSpec {
        name: "which",
        description: "Locate a command",
        relevance: 70.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["-a"],
            description: "Print all matches",
            relevance: 80.0,
        }],
    },
    CommandSpec {
        name: "ping",
        description: "Test network connectivity",
        relevance: 68.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-c"],
                description: "Number of packets",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-i"],
                description: "Interval between packets",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-t"],
                description: "TTL",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "netstat",
        description: "Network statistics",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-tuln"],
                description: "TCP/UDP listening ports (numeric)",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-an"],
                description: "All sockets numeric",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-p"],
                description: "Show program PID",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-r"],
                description: "Show routing table",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "lsof",
        description: "List open files and network connections",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-i"],
                description: "Select internet connections",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-p"],
                description: "Select by PID",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-n"],
                description: "No hostname resolution",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "systemctl",
        description: "Control systemd services",
        relevance: 70.0,
        subcommands: &[
            CommandSpec {
                name: "start",
                description: "Start a service",
                relevance: 100.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "stop",
                description: "Stop a service",
                relevance: 95.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "restart",
                description: "Restart a service",
                relevance: 92.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "status",
                description: "Show service status",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "enable",
                description: "Enable service at boot",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "disable",
                description: "Disable service at boot",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "reload",
                description: "Reload service configuration",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "daemon-reload",
                description: "Reload systemd manager",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "is-active",
                description: "Check if service is active",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "list-units",
                description: "List loaded units",
                relevance: 60.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--now"],
                description: "Activate/deactivate immediately",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--all"],
                description: "Show all units",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "apt",
        description: "Debian/Ubuntu package manager",
        relevance: 72.0,
        subcommands: &[
            CommandSpec {
                name: "install",
                description: "Install packages",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-y"],
                    description: "Yes to all prompts",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "remove",
                description: "Remove packages",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "purge",
                description: "Remove packages and config",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "update",
                description: "Update package index",
                relevance: 95.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "upgrade",
                description: "Upgrade installed packages",
                relevance: 90.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-y"],
                    description: "Yes to all prompts",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "search",
                description: "Search for packages",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "show",
                description: "Show package info",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "list",
                description: "List packages",
                relevance: 65.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--installed"],
                    description: "List installed only",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "autoremove",
                description: "Remove unused dependencies",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["-y", "--yes"],
                description: "Automatic yes",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-q", "--quiet"],
                description: "Quiet output",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "apt-get",
        description: "Debian/Ubuntu package manager (low-level)",
        relevance: 68.0,
        subcommands: &[
            CommandSpec {
                name: "install",
                description: "Install packages",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-y"],
                    description: "Yes to all prompts",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "remove",
                description: "Remove packages",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "update",
                description: "Update package index",
                relevance: 95.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "upgrade",
                description: "Upgrade packages",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "autoremove",
                description: "Remove unused packages",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "purge",
                description: "Remove with config",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[FlagSpec {
            names: &["-y", "--yes"],
            description: "Automatic yes",
            relevance: 80.0,
        }],
    },
    CommandSpec {
        name: "yum",
        description: "Red Hat / CentOS package manager",
        relevance: 65.0,
        subcommands: &[
            CommandSpec {
                name: "install",
                description: "Install packages",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-y"],
                    description: "Yes to all prompts",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "remove",
                description: "Remove packages",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "update",
                description: "Update packages",
                relevance: 95.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "search",
                description: "Search for packages",
                relevance: 75.0,
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
                name: "info",
                description: "Show package info",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[FlagSpec {
            names: &["-y"],
            description: "Automatic yes",
            relevance: 80.0,
        }],
    },
    CommandSpec {
        name: "dnf",
        description: "Fedora / RHEL 8+ package manager",
        relevance: 65.0,
        subcommands: &[
            CommandSpec {
                name: "install",
                description: "Install packages",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-y"],
                    description: "Yes to all prompts",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "remove",
                description: "Remove packages",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "update",
                description: "Update packages",
                relevance: 95.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "search",
                description: "Search for packages",
                relevance: 75.0,
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
                name: "autoremove",
                description: "Remove unused deps",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[FlagSpec {
            names: &["-y"],
            description: "Automatic yes",
            relevance: 80.0,
        }],
    },
    CommandSpec {
        name: "zip",
        description: "Create zip archives",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-r"],
                description: "Recurse into directories",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-9"],
                description: "Maximum compression",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-e"],
                description: "Encrypt with password",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["-x"],
                description: "Exclude files matching pattern",
                relevance: 55.0,
            },
        ],
    },
    CommandSpec {
        name: "unzip",
        description: "Extract zip archives",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-d"],
                description: "Extract to directory",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-l"],
                description: "List contents",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-o"],
                description: "Overwrite files",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "rsync",
        description: "Remote file synchronization",
        relevance: 68.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-avz"],
                description: "Archive, verbose, compress",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-a", "--archive"],
                description: "Archive mode",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Verbose output",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-z", "--compress"],
                description: "Compress during transfer",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-n", "--dry-run"],
                description: "Dry run",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["--delete"],
                description: "Delete extra files in dest",
                relevance: 60.0,
            },
            FlagSpec {
                names: &["--exclude"],
                description: "Exclude files",
                relevance: 55.0,
            },
            FlagSpec {
                names: &["-e"],
                description: "Remote shell command",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["-P"],
                description: "Partial + progress",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "crontab",
        description: "Manage cron jobs",
        relevance: 60.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-e"],
                description: "Edit crontab",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-l"],
                description: "List crontab",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-r"],
                description: "Remove crontab",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-u"],
                description: "Specify user",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "history",
        description: "Show command history",
        relevance: 70.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-c"],
                description: "Clear history",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-n"],
                description: "Read new entries from file",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "df",
        description: "Disk space usage",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-h", "--human-readable"],
                description: "Human readable sizes",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-T", "--print-type"],
                description: "Show filesystem type",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-a", "--all"],
                description: "Include all",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "du",
        description: "Disk usage of files",
        relevance: 63.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-sh"],
                description: "Summary in human readable",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-h", "--human-readable"],
                description: "Human readable",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-s", "--summarize"],
                description: "Summary only",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-d", "--max-depth"],
                description: "Max directory depth",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "top",
        description: "Display running processes",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-b"],
                description: "Batch mode",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-n"],
                description: "Number of iterations",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["-p"],
                description: "Monitor specific PID",
                relevance: 60.0,
            },
        ],
    },
    CommandSpec {
        name: "htop",
        description: "Interactive process viewer",
        relevance: 65.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-p"],
                description: "Monitor specific PID",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-u"],
                description: "Show processes for user",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "ln",
        description: "Create links",
        relevance: 62.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-s", "--symbolic"],
                description: "Create symbolic link",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["-f", "--force"],
                description: "Force remove existing",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Verbose output",
                relevance: 60.0,
            },
        ],
    },
];
