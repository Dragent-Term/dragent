//! Command specs shard 2 (generated split; content verbatim).
//!
//! Part of the SPECS table; see cmd_suggest::specs().

use super::cmd_suggest::{CommandSpec, FlagSpec};

pub(crate) const SPECS_02: &[CommandSpec] = &[
    CommandSpec {
        name: "docker",
        description: "Container platform",
        relevance: 100.0,
        subcommands: &[
            CommandSpec {
                name: "run",
                description: "Run a container",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-it"],
                        description: "Interactive TTY",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["-d"],
                        description: "Detached mode",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["-p"],
                        description: "Port mapping",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["-v"],
                        description: "Volume mount",
                        relevance: 75.0,
                    },
                    FlagSpec {
                        names: &["--rm"],
                        description: "Remove on exit",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--name"],
                        description: "Container name",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["-e"],
                        description: "Environment variable",
                        relevance: 65.0,
                    },
                    FlagSpec {
                        names: &["--network"],
                        description: "Network",
                        relevance: 50.0,
                    },
                ],
            },
            CommandSpec {
                name: "build",
                description: "Build an image",
                relevance: 95.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-t", "--tag"],
                        description: "Name and optionally tag",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["-f"],
                        description: "Dockerfile path",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--no-cache"],
                        description: "No build cache",
                        relevance: 60.0,
                    },
                    FlagSpec {
                        names: &["--platform"],
                        description: "Target platform",
                        relevance: 50.0,
                    },
                ],
            },
            CommandSpec {
                name: "ps",
                description: "List containers",
                relevance: 90.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-a", "--all"],
                    description: "Show all containers",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "exec",
                description: "Execute in container",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-it"],
                    description: "Interactive TTY",
                    relevance: 90.0,
                }],
            },
            CommandSpec {
                name: "images",
                description: "List images",
                relevance: 82.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-a", "--all"],
                    description: "Show all images",
                    relevance: 70.0,
                }],
            },
            CommandSpec {
                name: "pull",
                description: "Pull an image",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "push",
                description: "Push an image",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "stop",
                description: "Stop container",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "rm",
                description: "Remove container",
                relevance: 68.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-f", "--force"],
                    description: "Force remove",
                    relevance: 70.0,
                }],
            },
            CommandSpec {
                name: "rmi",
                description: "Remove image",
                relevance: 65.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-f"],
                    description: "Force remove",
                    relevance: 70.0,
                }],
            },
            CommandSpec {
                name: "logs",
                description: "Fetch container logs",
                relevance: 62.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-f", "--follow"],
                        description: "Follow log output",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--tail"],
                        description: "Last N lines",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "inspect",
                description: "Inspect container / image",
                relevance: 55.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "network",
                description: "Manage networks",
                relevance: 50.0,
                subcommands: &[
                    CommandSpec {
                        name: "ls",
                        description: "List networks",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "create",
                        description: "Create network",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "rm",
                        description: "Remove network",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "volume",
                description: "Manage volumes",
                relevance: 48.0,
                subcommands: &[
                    CommandSpec {
                        name: "ls",
                        description: "List volumes",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "create",
                        description: "Create volume",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "rm",
                        description: "Remove volume",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "prune",
                        description: "Remove unused volumes",
                        relevance: 60.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "compose",
                description: "Docker Compose (v2)",
                relevance: 88.0,
                subcommands: &[
                    CommandSpec {
                        name: "up",
                        description: "Create and start services",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[
                            FlagSpec {
                                names: &["-d", "--detach"],
                                description: "Detached mode",
                                relevance: 80.0,
                            },
                            FlagSpec {
                                names: &["--build"],
                                description: "Build images",
                                relevance: 70.0,
                            },
                        ],
                    },
                    CommandSpec {
                        name: "down",
                        description: "Stop and remove services",
                        relevance: 95.0,
                        subcommands: &[],
                        flags: &[FlagSpec {
                            names: &["-v", "--volumes"],
                            description: "Remove volumes",
                            relevance: 60.0,
                        }],
                    },
                    CommandSpec {
                        name: "build",
                        description: "Build services",
                        relevance: 85.0,
                        subcommands: &[],
                        flags: &[FlagSpec {
                            names: &["--no-cache"],
                            description: "No cache",
                            relevance: 60.0,
                        }],
                    },
                    CommandSpec {
                        name: "logs",
                        description: "View service logs",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[FlagSpec {
                            names: &["-f"],
                            description: "Follow logs",
                            relevance: 80.0,
                        }],
                    },
                    CommandSpec {
                        name: "ps",
                        description: "List services",
                        relevance: 75.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "restart",
                        description: "Restart services",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "exec",
                        description: "Execute in service",
                        relevance: 65.0,
                        subcommands: &[],
                        flags: &[FlagSpec {
                            names: &["-it"],
                            description: "Interactive TTY",
                            relevance: 90.0,
                        }],
                    },
                    CommandSpec {
                        name: "pull",
                        description: "Pull service images",
                        relevance: 60.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "system",
                description: "Manage Docker system",
                relevance: 40.0,
                subcommands: &[
                    CommandSpec {
                        name: "prune",
                        description: "Remove unused data",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[
                            FlagSpec {
                                names: &["-a", "--all"],
                                description: "Remove all",
                                relevance: 60.0,
                            },
                            FlagSpec {
                                names: &["-f"],
                                description: "No confirmation",
                                relevance: 50.0,
                            },
                        ],
                    },
                    CommandSpec {
                        name: "df",
                        description: "Show disk usage",
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
    // ── Docker Compose (legacy CLI) ────────────────────────────────────────────
    CommandSpec {
        name: "docker-compose",
        description: "Define and run multi-container applications",
        relevance: 90.0,
        subcommands: &[
            CommandSpec {
                name: "up",
                description: "Create and start services",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-d", "--detach"],
                        description: "Detached mode",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--build"],
                        description: "Build images",
                        relevance: 70.0,
                    },
                ],
            },
            CommandSpec {
                name: "down",
                description: "Stop and remove services",
                relevance: 95.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-v", "--volumes"],
                    description: "Remove volumes",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "build",
                description: "Build service images",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--no-cache"],
                    description: "No build cache",
                    relevance: 60.0,
                }],
            },
            CommandSpec {
                name: "logs",
                description: "View service logs",
                relevance: 80.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-f"],
                    description: "Follow logs",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "ps",
                description: "List services",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "restart",
                description: "Restart services",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "exec",
                description: "Execute in service",
                relevance: 65.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-it"],
                    description: "Interactive TTY",
                    relevance: 90.0,
                }],
            },
        ],
        flags: &[FlagSpec {
            names: &["-f"],
            description: "Compose file",
            relevance: 40.0,
        }],
    },
    // ── Kubernetes ────────────────────────────────────────────────────────────
    CommandSpec {
        name: "kubectl",
        description: "Kubernetes CLI",
        relevance: 100.0,
        subcommands: &[
            CommandSpec {
                name: "get",
                description: "Get resources",
                relevance: 100.0,
                subcommands: &[
                    CommandSpec {
                        name: "pods",
                        description: "List pods",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[
                            FlagSpec {
                                names: &["-A", "--all-namespaces"],
                                description: "All namespaces",
                                relevance: 70.0,
                            },
                            FlagSpec {
                                names: &["-w", "--watch"],
                                description: "Watch",
                                relevance: 60.0,
                            },
                        ],
                    },
                    CommandSpec {
                        name: "deployments",
                        description: "List deployments",
                        relevance: 95.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "services",
                        description: "List services",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "nodes",
                        description: "List nodes",
                        relevance: 85.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "namespaces",
                        description: "List namespaces",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "configmaps",
                        description: "List configmaps",
                        relevance: 75.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "secrets",
                        description: "List secrets",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "ingress",
                        description: "List ingress",
                        relevance: 65.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "pvc",
                        description: "List persistent volumes",
                        relevance: 60.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "all",
                        description: "Get all resources",
                        relevance: 55.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[
                    FlagSpec {
                        names: &["-n", "--namespace"],
                        description: "Target namespace",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["-o"],
                        description: "Output format",
                        relevance: 70.0,
                    },
                ],
            },
            CommandSpec {
                name: "apply",
                description: "Apply configuration",
                relevance: 95.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-f", "--filename"],
                        description: "File or directory",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["-k"],
                        description: "Kustomize dir",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "delete",
                description: "Delete resources",
                relevance: 88.0,
                subcommands: &[
                    CommandSpec {
                        name: "pod",
                        description: "Delete pod",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "deployment",
                        description: "Delete deployment",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "service",
                        description: "Delete service",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[
                    FlagSpec {
                        names: &["-f", "--filename"],
                        description: "File",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--force"],
                        description: "Force delete",
                        relevance: 40.0,
                    },
                ],
            },
            CommandSpec {
                name: "describe",
                description: "Show resource details",
                relevance: 85.0,
                subcommands: &[
                    CommandSpec {
                        name: "pod",
                        description: "Describe pod",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "node",
                        description: "Describe node",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "deployment",
                        description: "Describe deployment",
                        relevance: 85.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "logs",
                description: "Print pod logs",
                relevance: 82.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-f", "--follow"],
                        description: "Follow logs",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["-n"],
                        description: "Namespace",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--previous", "-p"],
                        description: "Previous container",
                        relevance: 50.0,
                    },
                    FlagSpec {
                        names: &["--tail"],
                        description: "Last N lines",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "exec",
                description: "Execute in pod",
                relevance: 78.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-it"],
                        description: "Interactive TTY",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["-n"],
                        description: "Namespace",
                        relevance: 70.0,
                    },
                    FlagSpec {
                        names: &["--"],
                        description: "Command separator",
                        relevance: 60.0,
                    },
                ],
            },
            CommandSpec {
                name: "port-forward",
                description: "Forward ports",
                relevance: 72.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-n"],
                    description: "Namespace",
                    relevance: 70.0,
                }],
            },
            CommandSpec {
                name: "rollout",
                description: "Manage rollouts",
                relevance: 68.0,
                subcommands: &[
                    CommandSpec {
                        name: "status",
                        description: "Show rollout status",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "restart",
                        description: "Restart a resource",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "undo",
                        description: "Undo a rollout",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "scale",
                description: "Scale a resource",
                relevance: 60.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--replicas"],
                    description: "Number of replicas",
                    relevance: 90.0,
                }],
            },
            CommandSpec {
                name: "config",
                description: "Modify kubeconfig",
                relevance: 55.0,
                subcommands: &[
                    CommandSpec {
                        name: "get-contexts",
                        description: "List contexts",
                        relevance: 100.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "use-context",
                        description: "Set context",
                        relevance: 95.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "current-context",
                        description: "Current context",
                        relevance: 90.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "set-context",
                        description: "Set context value",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[],
            },
            CommandSpec {
                name: "create",
                description: "Create resource",
                relevance: 50.0,
                subcommands: &[
                    CommandSpec {
                        name: "namespace",
                        description: "Create namespace",
                        relevance: 80.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "secret",
                        description: "Create secret",
                        relevance: 75.0,
                        subcommands: &[],
                        flags: &[],
                    },
                    CommandSpec {
                        name: "deployment",
                        description: "Create deployment",
                        relevance: 70.0,
                        subcommands: &[],
                        flags: &[],
                    },
                ],
                flags: &[FlagSpec {
                    names: &["-f"],
                    description: "File",
                    relevance: 80.0,
                }],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["-n", "--namespace"],
                description: "Namespace",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--context"],
                description: "Context",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--kubeconfig"],
                description: "Config file",
                relevance: 50.0,
            },
        ],
    },
    // ── AWS CLI ───────────────────────────────────────────────────────────────
];
