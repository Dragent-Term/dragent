//! Command specs shard 9 (generated split; content verbatim).
//!
//! Part of the SPECS table; see cmd_suggest::specs().

use super::cmd_suggest::{CommandSpec, FlagSpec};

pub(crate) const SPECS_09: &[CommandSpec] = &[
    CommandSpec {
        name: "doctl",
        description: "DigitalOcean CLI",
        relevance: 78.0,
        subcommands: &[
            CommandSpec {
                name: "compute droplet list",
                description: "List Droplets",
                relevance: 100.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "compute droplet create",
                description: "Create a Droplet",
                relevance: 95.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--size"],
                        description: "Droplet size slug",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--image"],
                        description: "Droplet image slug",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--region"],
                        description: "Region slug",
                        relevance: 85.0,
                    },
                ],
            },
            CommandSpec {
                name: "compute droplet delete",
                description: "Delete a Droplet",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "kubernetes cluster list",
                description: "List DOKS clusters",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "kubernetes cluster create",
                description: "Create a Kubernetes cluster",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "kubernetes cluster kubeconfig save",
                description: "Save cluster kubeconfig",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "databases list",
                description: "List managed databases",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "apps list",
                description: "List App Platform apps",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "apps create",
                description: "Create an App Platform app",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "registry list",
                description: "List container registries",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "auth init",
                description: "Authenticate with DigitalOcean",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--format"],
                description: "Output format",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--no-header"],
                description: "Remove table header",
                relevance: 60.0,
            },
        ],
    },
    // ── Databases ─────────────────────────────────────────────────────────────
    CommandSpec {
        name: "psql",
        description: "PostgreSQL interactive terminal",
        relevance: 85.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-h", "--host"],
                description: "Database host",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-p", "--port"],
                description: "Database port",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-U", "--username"],
                description: "Database user",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-d", "--dbname"],
                description: "Database name",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["-c", "--command"],
                description: "Run single SQL command",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-f", "--file"],
                description: "Execute SQL from file",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-W", "--password"],
                description: "Prompt for password",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-l", "--list"],
                description: "List available databases",
                relevance: 78.0,
            },
        ],
    },
    CommandSpec {
        name: "pg_dump",
        description: "Dump a PostgreSQL database",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-h", "--host"],
                description: "Database host",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-U", "--username"],
                description: "Database user",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-d", "--dbname"],
                description: "Database name",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-F", "--format"],
                description: "Format: c=custom p=plain t=tar d=dir",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-f", "--file"],
                description: "Output file",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-n", "--schema"],
                description: "Dump this schema only",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-t", "--table"],
                description: "Dump this table only",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--no-owner"],
                description: "Skip ownership commands",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-j", "--jobs"],
                description: "Parallel dump jobs",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "mysql",
        description: "MySQL command-line client",
        relevance: 82.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-h", "--host"],
                description: "Database host",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-P", "--port"],
                description: "Database port",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["-u", "--user"],
                description: "Database user",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["-p", "--password"],
                description: "Password (or prompt)",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-D", "--database"],
                description: "Database name",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-e", "--execute"],
                description: "Execute SQL statement",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-B", "--batch"],
                description: "Non-interactive batch mode",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--ssl-ca"],
                description: "CA certificate file",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "mongosh",
        description: "MongoDB Shell (modern)",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--host"],
                description: "MongoDB host",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--port"],
                description: "MongoDB port",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--username", "-u"],
                description: "Username",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--password", "-p"],
                description: "Password",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--authenticationDatabase"],
                description: "Auth database",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--eval"],
                description: "Evaluate JavaScript expression",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--tls"],
                description: "Use TLS/SSL",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "redis-cli",
        description: "Redis command-line interface",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-h"],
                description: "Server hostname",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-p"],
                description: "Server port",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-a"],
                description: "Password",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-n"],
                description: "Database number",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-u"],
                description: "Server URI (redis://...)",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--tls"],
                description: "Use TLS",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--cluster"],
                description: "Cluster mode command",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["-c"],
                description: "Enable cluster mode",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["--scan"],
                description: "Scan key space",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--pipe"],
                description: "Read commands from stdin",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["--stat"],
                description: "Print rolling stats",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--latency"],
                description: "Show latency stats",
                relevance: 68.0,
            },
        ],
    },
    // ── Security / scanning tools ─────────────────────────────────────────────
    CommandSpec {
        name: "trivy",
        description: "Aqua Security vulnerability and misconfiguration scanner",
        relevance: 82.0,
        subcommands: &[
            CommandSpec {
                name: "image",
                description: "Scan a container image",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--severity"],
                        description: "Filter by severity (CRITICAL,HIGH,...)",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["--format"],
                        description: "Output format (table/json/sarif)",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--exit-code"],
                        description: "Exit code when vulnerabilities found",
                        relevance: 75.0,
                    },
                ],
            },
            CommandSpec {
                name: "fs",
                description: "Scan filesystem or directory",
                relevance: 92.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--severity"],
                    description: "Severity filter",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "repo",
                description: "Scan a Git repository",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "config",
                description: "Scan IaC config files for misconfigurations",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "k8s",
                description: "Scan Kubernetes cluster",
                relevance: 82.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--all-namespaces"],
                    description: "Scan all namespaces",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "sbom",
                description: "Generate software bill of materials",
                relevance: 78.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--format"],
                    description: "SBOM format (cyclonedx/spdx)",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "secret",
                description: "Scan for exposed secrets",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--severity"],
                description: "Severity levels to report",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--format", "-f"],
                description: "Output format",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--exit-code"],
                description: "Exit code on findings",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--ignorefile"],
                description: ".trivyignore file path",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "snyk",
        description: "Snyk security vulnerability scanner",
        relevance: 80.0,
        subcommands: &[
            CommandSpec {
                name: "test",
                description: "Test project for vulnerabilities",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--severity-threshold"],
                        description: "Min severity (low/medium/high/critical)",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--json"],
                        description: "JSON output",
                        relevance: 75.0,
                    },
                ],
            },
            CommandSpec {
                name: "monitor",
                description: "Snapshot and monitor project",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "fix",
                description: "Auto-fix vulnerabilities",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "container test",
                description: "Scan container image",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--severity-threshold"],
                    description: "Min severity",
                    relevance: 82.0,
                }],
            },
            CommandSpec {
                name: "container monitor",
                description: "Monitor container image",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "iac test",
                description: "Scan IaC files for issues",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "code test",
                description: "Scan source code with SAST",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "auth",
                description: "Authenticate with Snyk",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--org"],
                description: "Snyk organization slug",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--json"],
                description: "JSON output",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "checkov",
        description: "Bridgecrew Checkov — IaC static analysis",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-d", "--directory"],
                description: "Scan directory",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["-f", "--file"],
                description: "Scan a single file",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--framework"],
                description: "Framework to scan (terraform/cloudformation/kubernetes/...)",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--check"],
                description: "Run only specific check IDs",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--skip-check"],
                description: "Skip specific check IDs",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["-o", "--output"],
                description: "Output format (cli/json/junitxml/sarif)",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--soft-fail"],
                description: "Return exit code 0 even on failures",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "tflint",
        description: "Terraform linter",
        relevance: 75.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--init"],
                description: "Install plugins",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--config"],
                description: "Path to .tflint.hcl",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--chdir"],
                description: "Change working directory",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--format"],
                description: "Output format (default/json/checkstyle/sarif)",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--only"],
                description: "Enable only specified rule",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--var-file"],
                description: "Terraform variables file",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "hadolint",
        description: "Dockerfile linter",
        relevance: 75.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-f", "--format"],
                description: "Output format (tty/json/checkstyle/codeclimate/sarif)",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--ignore"],
                description: "Ignore specific rule",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--failure-threshold"],
                description: "Min severity to fail (error/warning/info/style)",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--no-fail"],
                description: "Always exit 0",
                relevance: 65.0,
            },
        ],
    },
    // ── Networking / TLS ──────────────────────────────────────────────────────
    CommandSpec {
        name: "dig",
        description: "DNS lookup utility",
        relevance: 80.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["+short"],
                description: "Terse output",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["+trace"],
                description: "Trace delegation path from root",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["+noall", "+answer"],
                description: "Show only answer section",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["-t"],
                description: "Query type (A/AAAA/MX/TXT/NS/CNAME/SOA)",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-x"],
                description: "Reverse lookup",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["@"],
                description: "Query specific nameserver",
                relevance: 85.0,
            },
        ],
    },
    CommandSpec {
        name: "nmap",
        description: "Network exploration and security scanning",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-sV"],
                description: "Service/version detection",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["-sS"],
                description: "TCP SYN scan (stealth)",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-sU"],
                description: "UDP scan",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["-p"],
                description: "Port range to scan",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-A"],
                description: "Aggressive scan (OS + version + scripts)",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-O"],
                description: "OS detection",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["-oN"],
                description: "Output to file (normal format)",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-oX"],
                description: "Output to file (XML format)",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--open"],
                description: "Show only open ports",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-v"],
                description: "Verbose output",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["-Pn"],
                description: "Skip host discovery (no ping)",
                relevance: 78.0,
            },
        ],
    },
    CommandSpec {
        name: "openssl",
        description: "OpenSSL cryptographic toolkit",
        relevance: 80.0,
        subcommands: &[
            CommandSpec {
                name: "req -new -x509",
                description: "Generate self-signed certificate",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-keyout"],
                        description: "Private key output file",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["-out"],
                        description: "Certificate output file",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["-days"],
                        description: "Certificate validity in days",
                        relevance: 85.0,
                    },
                ],
            },
            CommandSpec {
                name: "req -new",
                description: "Generate certificate signing request",
                relevance: 92.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-key"],
                        description: "Private key file",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["-out"],
                        description: "CSR output file",
                        relevance: 85.0,
                    },
                ],
            },
            CommandSpec {
                name: "genrsa",
                description: "Generate RSA private key",
                relevance: 90.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-out"],
                    description: "Output file",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "x509 -in",
                description: "Display certificate info",
                relevance: 88.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-text"],
                        description: "Full text output",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["-noout"],
                        description: "Suppress encoded output",
                        relevance: 80.0,
                    },
                ],
            },
            CommandSpec {
                name: "s_client -connect",
                description: "TLS handshake debugging",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-showcerts"],
                    description: "Show full certificate chain",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "enc",
                description: "Encrypt/decrypt files",
                relevance: 80.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-d"],
                        description: "Decrypt mode",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["-in"],
                        description: "Input file",
                        relevance: 82.0,
                    },
                    FlagSpec {
                        names: &["-out"],
                        description: "Output file",
                        relevance: 82.0,
                    },
                ],
            },
            CommandSpec {
                name: "dgst",
                description: "Compute message digest/HMAC",
                relevance: 78.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-sha256"],
                    description: "Use SHA-256 digest",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "verify",
                description: "Verify certificate",
                relevance: 75.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-CAfile"],
                    description: "CA certificate file",
                    relevance: 82.0,
                }],
            },
        ],
        flags: &[],
    },
    CommandSpec {
        name: "scp",
        description: "Secure copy (remote file transfer via SSH)",
        relevance: 80.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-r"],
                description: "Recursively copy directories",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-P"],
                description: "Remote host port",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-i"],
                description: "Identity file (private key)",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["-p"],
                description: "Preserve file modification times",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["-C"],
                description: "Enable compression",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["-v"],
                description: "Verbose output",
                relevance: 68.0,
            },
        ],
    },
    CommandSpec {
        name: "certbot",
        description: "Let's Encrypt SSL certificate tool",
        relevance: 78.0,
        subcommands: &[
            CommandSpec {
                name: "certonly",
                description: "Obtain certificate only (no install)",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--standalone"],
                        description: "Built-in web server",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--nginx"],
                        description: "Use Nginx plugin",
                        relevance: 82.0,
                    },
                    FlagSpec {
                        names: &["--dns-route53"],
                        description: "Use Route53 DNS challenge",
                        relevance: 78.0,
                    },
                ],
            },
            CommandSpec {
                name: "renew",
                description: "Renew near-expiry certificates",
                relevance: 95.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--dry-run"],
                    description: "Test without saving",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "certificates",
                description: "List obtained certificates",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "delete",
                description: "Delete a certificate",
                relevance: 78.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--cert-name"],
                    description: "Certificate name",
                    relevance: 88.0,
                }],
            },
            CommandSpec {
                name: "nginx",
                description: "Obtain and install Nginx certificate",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-d", "--domain"],
                    description: "Domain name",
                    relevance: 92.0,
                }],
            },
            CommandSpec {
                name: "apache",
                description: "Obtain and install Apache certificate",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["-d", "--domain"],
                description: "Domain name(s)",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["--dry-run"],
                description: "Test run without changes",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--agree-tos"],
                description: "Agree to ACME terms",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--email"],
                description: "Contact email for registration",
                relevance: 78.0,
            },
        ],
    },
    // ── Monitoring / load testing ─────────────────────────────────────────────
    CommandSpec {
        name: "k6",
        description: "k6 load testing tool",
        relevance: 78.0,
        subcommands: &[
            CommandSpec {
                name: "run",
                description: "Run a load test script",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--vus", "-u"],
                        description: "Number of virtual users",
                        relevance: 92.0,
                    },
                    FlagSpec {
                        names: &["--duration", "-d"],
                        description: "Test duration (e.g. 30s, 5m)",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["--out", "-o"],
                        description: "Output results to backend",
                        relevance: 78.0,
                    },
                    FlagSpec {
                        names: &["--env", "-e"],
                        description: "Set environment variable",
                        relevance: 75.0,
                    },
                ],
            },
            CommandSpec {
                name: "cloud",
                description: "Run test on k6 Cloud",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "pause",
                description: "Pause a running test",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "resume",
                description: "Resume a paused test",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "scale",
                description: "Scale VUs of a running test",
                relevance: 68.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "stats",
                description: "Show running test stats",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--vus", "-u"],
                description: "Number of virtual users",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--duration", "-d"],
                description: "Test duration",
                relevance: 88.0,
            },
        ],
    },
    // ── Secret management ─────────────────────────────────────────────────────
    CommandSpec {
        name: "op",
        description: "1Password CLI — secrets and credential management",
        relevance: 78.0,
        subcommands: &[
            CommandSpec {
                name: "item get",
                description: "Get an item from 1Password",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--fields"],
                        description: "Fields to return",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--format"],
                        description: "Output format (json)",
                        relevance: 75.0,
                    },
                ],
            },
            CommandSpec {
                name: "item list",
                description: "List items in vault",
                relevance: 92.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--vault"],
                    description: "Vault to list from",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "item create",
                description: "Create a new item",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "item edit",
                description: "Edit an existing item",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "item delete",
                description: "Move item to Trash",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "vault list",
                description: "List vaults",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "run",
                description: "Inject secrets as env vars and run command",
                relevance: 90.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--env-file"],
                    description: "Env file with op:// references",
                    relevance: 88.0,
                }],
            },
            CommandSpec {
                name: "inject",
                description: "Inject secrets from stdin template",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "read",
                description: "Read secret reference (op://...)",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "signin",
                description: "Sign in to 1Password",
                relevance: 72.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "signout",
                description: "Sign out of 1Password",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--account"],
                description: "1Password account",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--format"],
                description: "Output format (human-readable/json)",
                relevance: 70.0,
            },
        ],
    },
    // ── Object storage ────────────────────────────────────────────────────────
    CommandSpec {
        name: "mc",
        description: "MinIO Client — S3-compatible object storage CLI",
        relevance: 75.0,
        subcommands: &[
            CommandSpec {
                name: "ls",
                description: "List buckets and objects",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--recursive", "-r"],
                    description: "List recursively",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "cp",
                description: "Copy objects",
                relevance: 95.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--recursive", "-r"],
                    description: "Copy recursively",
                    relevance: 82.0,
                }],
            },
            CommandSpec {
                name: "mv",
                description: "Move objects",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "rm",
                description: "Remove objects",
                relevance: 82.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--recursive", "-r"],
                        description: "Remove recursively",
                        relevance: 82.0,
                    },
                    FlagSpec {
                        names: &["--force"],
                        description: "Skip confirmation",
                        relevance: 78.0,
                    },
                ],
            },
            CommandSpec {
                name: "mirror",
                description: "Mirror buckets/folders",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--watch", "-w"],
                    description: "Watch and mirror continuously",
                    relevance: 78.0,
                }],
            },
            CommandSpec {
                name: "mb",
                description: "Make a bucket",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "rb",
                description: "Remove a bucket",
                relevance: 72.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--force"],
                    description: "Force remove non-empty bucket",
                    relevance: 78.0,
                }],
            },
            CommandSpec {
                name: "cat",
                description: "Display object contents",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "stat",
                description: "Show object metadata",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "alias set",
                description: "Configure a new alias (endpoint)",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "alias list",
                description: "List configured aliases",
                relevance: 68.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "admin info",
                description: "Show MinIO server info",
                relevance: 72.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[],
    },
    // ── Kubernetes context/namespace switchers ────────────────────────────────
    CommandSpec {
        name: "kubectx",
        description: "Switch between Kubernetes contexts",
        relevance: 82.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-d"],
                description: "Delete a context",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-c", "--current"],
                description: "Show current context",
                relevance: 80.0,
            },
        ],
    },
    CommandSpec {
        name: "kubens",
        description: "Switch between Kubernetes namespaces",
        relevance: 82.0,
        subcommands: &[],
        flags: &[FlagSpec {
            names: &["-c", "--current"],
            description: "Show current namespace",
            relevance: 80.0,
        }],
    },
    // ── AWS ecosystem extras ──────────────────────────────────────────────────
];
