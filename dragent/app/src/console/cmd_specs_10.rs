//! Command specs shard 10 (generated split; content verbatim).
//!
//! Part of the SPECS table; see cmd_suggest::specs().

use super::cmd_suggest::{CommandSpec, FlagSpec};

pub(crate) const SPECS_10: &[CommandSpec] = &[
    CommandSpec {
        name: "eksctl",
        description: "AWS EKS cluster management CLI",
        relevance: 82.0,
        subcommands: &[
            CommandSpec {
                name: "create cluster",
                description: "Create an EKS cluster",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--name"],
                        description: "Cluster name",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["--region"],
                        description: "AWS region",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["--nodegroup-name"],
                        description: "Nodegroup name",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--node-type"],
                        description: "EC2 instance type",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--nodes"],
                        description: "Number of nodes",
                        relevance: 78.0,
                    },
                ],
            },
            CommandSpec {
                name: "delete cluster",
                description: "Delete an EKS cluster",
                relevance: 88.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--name"],
                        description: "Cluster name",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["--region"],
                        description: "AWS region",
                        relevance: 85.0,
                    },
                ],
            },
            CommandSpec {
                name: "get cluster",
                description: "List EKS clusters",
                relevance: 92.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--region"],
                    description: "AWS region",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "get nodegroup",
                description: "List nodegroups",
                relevance: 88.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--cluster"],
                    description: "Cluster name",
                    relevance: 90.0,
                }],
            },
            CommandSpec {
                name: "scale nodegroup",
                description: "Scale nodegroup",
                relevance: 85.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--cluster"],
                        description: "Cluster name",
                        relevance: 90.0,
                    },
                    FlagSpec {
                        names: &["--nodes"],
                        description: "Desired node count",
                        relevance: 88.0,
                    },
                ],
            },
            CommandSpec {
                name: "create nodegroup",
                description: "Add a nodegroup to cluster",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "delete nodegroup",
                description: "Remove a nodegroup",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "utils write-kubeconfig",
                description: "Write kubeconfig for cluster",
                relevance: 82.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--cluster"],
                    description: "Cluster name",
                    relevance: 90.0,
                }],
            },
            CommandSpec {
                name: "upgrade cluster",
                description: "Upgrade EKS cluster",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--region"],
                description: "AWS region",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--profile"],
                description: "AWS profile",
                relevance: 80.0,
            },
        ],
    },
    CommandSpec {
        name: "sam",
        description: "AWS Serverless Application Model CLI",
        relevance: 80.0,
        subcommands: &[
            CommandSpec {
                name: "build",
                description: "Build Lambda functions",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--use-container"],
                    description: "Build inside Docker container",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "deploy",
                description: "Deploy SAM application",
                relevance: 98.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--guided"],
                        description: "Interactive guided deployment",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["--stack-name"],
                        description: "CloudFormation stack name",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--region"],
                        description: "AWS region",
                        relevance: 82.0,
                    },
                ],
            },
            CommandSpec {
                name: "local invoke",
                description: "Invoke Lambda function locally",
                relevance: 92.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-e", "--event"],
                    description: "Event JSON file",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "local start-api",
                description: "Start local API Gateway",
                relevance: 88.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-p", "--port"],
                    description: "Local port",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "local start-lambda",
                description: "Start local Lambda endpoint",
                relevance: 83.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "init",
                description: "Initialize a new SAM project",
                relevance: 80.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--runtime"],
                    description: "Lambda runtime",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "logs",
                description: "Fetch CloudWatch logs for Lambda",
                relevance: 78.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["-n", "--name"],
                        description: "Function name",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["-t", "--tail"],
                        description: "Stream logs",
                        relevance: 82.0,
                    },
                ],
            },
            CommandSpec {
                name: "validate",
                description: "Validate SAM template",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "delete",
                description: "Delete deployed application",
                relevance: 75.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--stack-name"],
                    description: "Stack name",
                    relevance: 88.0,
                }],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--region"],
                description: "AWS region",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--profile"],
                description: "AWS CLI profile",
                relevance: 80.0,
            },
        ],
    },
    // ── JavaScript / TypeScript tooling ───────────────────────────────────────
    CommandSpec {
        name: "vite",
        description: "Vite — fast frontend build tool",
        relevance: 88.0,
        subcommands: &[
            CommandSpec {
                name: "dev",
                description: "Start development server",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--port"],
                        description: "Dev server port",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--host"],
                        description: "Expose to network",
                        relevance: 78.0,
                    },
                    FlagSpec {
                        names: &["--open"],
                        description: "Open browser",
                        relevance: 70.0,
                    },
                ],
            },
            CommandSpec {
                name: "build",
                description: "Build for production",
                relevance: 95.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--mode"],
                        description: "Build mode (production/staging)",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--outDir"],
                        description: "Output directory",
                        relevance: 75.0,
                    },
                ],
            },
            CommandSpec {
                name: "preview",
                description: "Preview production build locally",
                relevance: 88.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--port"],
                    description: "Preview port",
                    relevance: 80.0,
                }],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--config", "-c"],
                description: "Config file path",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--mode", "-m"],
                description: "Environment mode",
                relevance: 78.0,
            },
        ],
    },
    CommandSpec {
        name: "jest",
        description: "Jest JavaScript test runner",
        relevance: 85.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--watch"],
                description: "Watch mode",
                relevance: 92.0,
            },
            FlagSpec {
                names: &["--watchAll"],
                description: "Watch all files",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--coverage"],
                description: "Collect and report coverage",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--testPathPattern", "-t"],
                description: "Run tests matching pattern",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--runInBand"],
                description: "Run serially (useful for debugging)",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--verbose"],
                description: "Verbose output",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--clearCache"],
                description: "Clear Jest cache",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--bail", "-b"],
                description: "Stop after N failures",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--passWithNoTests"],
                description: "Exit 0 when no test files found",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["--forceExit"],
                description: "Force exit after tests complete",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["--updateSnapshot", "-u"],
                description: "Update failing snapshots",
                relevance: 75.0,
            },
        ],
    },
    CommandSpec {
        name: "tsc",
        description: "TypeScript compiler",
        relevance: 85.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--watch", "-w"],
                description: "Watch mode",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--noEmit"],
                description: "Type-check only, no output",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--strict"],
                description: "Enable strict type checking",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--outDir"],
                description: "Output directory",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--target"],
                description: "ECMAScript target version",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--module"],
                description: "Module system (commonjs/esm...)",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--declaration"],
                description: "Generate .d.ts files",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--project", "-p"],
                description: "Path to tsconfig.json",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--init"],
                description: "Create default tsconfig.json",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--skipLibCheck"],
                description: "Skip type checking of declaration files",
                relevance: 68.0,
            },
        ],
    },
    CommandSpec {
        name: "eslint",
        description: "JavaScript / TypeScript linter",
        relevance: 82.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--fix"],
                description: "Auto-fix fixable problems",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["--ext"],
                description: "File extensions to lint",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--rule"],
                description: "Specify rule (as JSON)",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--ignore-path"],
                description: "Path to ignore file",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--no-eslintrc"],
                description: "Disable config file loading",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["--format", "-f"],
                description: "Output format",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--output-file", "-o"],
                description: "Write output to file",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["--quiet", "-q"],
                description: "Report errors only",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--max-warnings"],
                description: "Exit non-zero if warnings exceed threshold",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["--cache"],
                description: "Cache lint results",
                relevance: 72.0,
            },
        ],
    },
    CommandSpec {
        name: "prettier",
        description: "Opinionated code formatter",
        relevance: 82.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--write"],
                description: "Edit files in place",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["--check"],
                description: "Check if files are formatted (no write)",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--parser"],
                description: "Force parser (babel/typescript/css/...)",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--single-quote"],
                description: "Use single quotes",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--tab-width"],
                description: "Tab width",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["--trailing-comma"],
                description: "Trailing comma style",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["--print-width"],
                description: "Line wrap length",
                relevance: 65.0,
            },
            FlagSpec {
                names: &["--ignore-path"],
                description: "Path to ignore file",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "webpack",
        description: "Webpack module bundler",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--mode"],
                description: "Build mode (development/production/none)",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--config", "-c"],
                description: "Config file path",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--watch", "-w"],
                description: "Watch mode",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--entry"],
                description: "Entry point",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--output-path", "-o"],
                description: "Output directory",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--devtool"],
                description: "Source map type",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--progress"],
                description: "Show build progress",
                relevance: 70.0,
            },
            FlagSpec {
                names: &["--env"],
                description: "Pass environment variables",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--analyze"],
                description: "Analyze bundle (with plugin)",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "esbuild",
        description: "Extremely fast JavaScript bundler",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--bundle"],
                description: "Bundle all dependencies",
                relevance: 95.0,
            },
            FlagSpec {
                names: &["--outfile"],
                description: "Output file",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--outdir"],
                description: "Output directory",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--platform"],
                description: "Target platform (browser/node/neutral)",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--target"],
                description: "Target environment",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--minify"],
                description: "Minify output",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--sourcemap"],
                description: "Generate source map",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--watch"],
                description: "Rebuild on file changes",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--serve"],
                description: "Serve output files over HTTP",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--loader"],
                description: "Loader for file type",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--external"],
                description: "Mark package as external",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--format"],
                description: "Output format (iife/cjs/esm)",
                relevance: 80.0,
            },
        ],
    },
    CommandSpec {
        name: "rollup",
        description: "Rollup JavaScript module bundler",
        relevance: 75.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-c", "--config"],
                description: "Config file (default rollup.config.js)",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-w", "--watch"],
                description: "Watch mode",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-i", "--input"],
                description: "Entry point",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-o", "--file"],
                description: "Output file",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["-f", "--format"],
                description: "Output format (es/cjs/umd/iife)",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-n", "--name"],
                description: "UMD/IIFE bundle name",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-m", "--sourcemap"],
                description: "Generate source map",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["-p", "--plugin"],
                description: "Use plugin",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "playwright",
        description: "Playwright E2E browser testing",
        relevance: 80.0,
        subcommands: &[
            CommandSpec {
                name: "test",
                description: "Run Playwright tests",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--headed"],
                        description: "Run in headed mode",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["--project"],
                        description: "Run specific project",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--grep", "-g"],
                        description: "Filter tests by title",
                        relevance: 82.0,
                    },
                    FlagSpec {
                        names: &["--debug"],
                        description: "Run in debug mode",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--workers", "-j"],
                        description: "Number of parallel workers",
                        relevance: 78.0,
                    },
                ],
            },
            CommandSpec {
                name: "show-report",
                description: "Open HTML test report",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "codegen",
                description: "Record and generate test code",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--browser"],
                    description: "Browser type (chromium/firefox/webkit)",
                    relevance: 80.0,
                }],
            },
            CommandSpec {
                name: "install",
                description: "Install browsers",
                relevance: 80.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--with-deps"],
                    description: "Install OS dependencies",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "trace",
                description: "Show trace file",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[FlagSpec {
            names: &["--config"],
            description: "Path to playwright.config.ts",
            relevance: 75.0,
        }],
    },
    CommandSpec {
        name: "cypress",
        description: "Cypress E2E testing framework",
        relevance: 80.0,
        subcommands: &[
            CommandSpec {
                name: "open",
                description: "Open Cypress Test Runner",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--browser", "-b"],
                        description: "Browser to open",
                        relevance: 82.0,
                    },
                    FlagSpec {
                        names: &["--e2e"],
                        description: "Open for E2E testing",
                        relevance: 78.0,
                    },
                    FlagSpec {
                        names: &["--component"],
                        description: "Open for component testing",
                        relevance: 75.0,
                    },
                ],
            },
            CommandSpec {
                name: "run",
                description: "Run Cypress tests headlessly",
                relevance: 95.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--browser", "-b"],
                        description: "Browser to run",
                        relevance: 85.0,
                    },
                    FlagSpec {
                        names: &["--spec", "-s"],
                        description: "Spec file(s) to run",
                        relevance: 88.0,
                    },
                    FlagSpec {
                        names: &["--headless"],
                        description: "Run headlessly",
                        relevance: 80.0,
                    },
                    FlagSpec {
                        names: &["--record"],
                        description: "Record results to Cypress Cloud",
                        relevance: 72.0,
                    },
                ],
            },
            CommandSpec {
                name: "install",
                description: "Install Cypress binary",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "verify",
                description: "Verify Cypress is installed",
                relevance: 68.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "info",
                description: "Print debug info",
                relevance: 65.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--config", "-c"],
                description: "Inline config overrides",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--env", "-e"],
                description: "Set environment variables",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "mocha",
        description: "Mocha JavaScript test framework",
        relevance: 75.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--watch", "-w"],
                description: "Watch mode",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["--grep", "-g"],
                description: "Filter tests by pattern",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--timeout", "-t"],
                description: "Test timeout in ms",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--reporter", "-R"],
                description: "Output reporter",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--bail", "-b"],
                description: "Stop after first failure",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--recursive"],
                description: "Include subdirectories",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--exit"],
                description: "Force exit after tests",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["--require", "-r"],
                description: "Require module before tests",
                relevance: 70.0,
            },
        ],
    },
    // ── Python tooling ────────────────────────────────────────────────────────
    CommandSpec {
        name: "pytest",
        description: "Python test runner",
        relevance: 85.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-v", "--verbose"],
                description: "Verbose output",
                relevance: 90.0,
            },
            FlagSpec {
                names: &["-s", "--capture=no"],
                description: "Disable output capture",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["-k"],
                description: "Filter tests by keyword",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-x", "--exitfirst"],
                description: "Stop after first failure",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--lf", "--last-failed"],
                description: "Run only last-failed tests",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--ff", "--failed-first"],
                description: "Run failed tests first",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--cov"],
                description: "Collect coverage (pytest-cov)",
                relevance: 85.0,
            },
            FlagSpec {
                names: &["--cov-report"],
                description: "Coverage report format",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--maxfail"],
                description: "Stop after N failures",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-n", "--numprocesses"],
                description: "Parallel test workers (pytest-xdist)",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["-m"],
                description: "Filter by marker expression",
                relevance: 80.0,
            },
            FlagSpec {
                names: &["--tb"],
                description: "Traceback style (short/long/no/line)",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--pdb"],
                description: "Drop to pdb on failure",
                relevance: 70.0,
            },
        ],
    },
    CommandSpec {
        name: "pyenv",
        description: "Python version manager",
        relevance: 80.0,
        subcommands: &[
            CommandSpec {
                name: "install",
                description: "Install a Python version",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-l", "--list"],
                    description: "List available versions",
                    relevance: 88.0,
                }],
            },
            CommandSpec {
                name: "global",
                description: "Set global Python version",
                relevance: 95.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "local",
                description: "Set local Python version (.python-version)",
                relevance: 92.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "shell",
                description: "Set shell Python version",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "versions",
                description: "List installed versions",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "version",
                description: "Show active Python version",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "which",
                description: "Show full path to executable",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "rehash",
                description: "Rehash shims after install",
                relevance: 70.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "uninstall",
                description: "Remove a Python version",
                relevance: 72.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[],
    },
    CommandSpec {
        name: "pipenv",
        description: "Python virtualenv and dependency manager",
        relevance: 78.0,
        subcommands: &[
            CommandSpec {
                name: "install",
                description: "Install packages and add to Pipfile",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--dev"],
                    description: "Install as dev dependency",
                    relevance: 85.0,
                }],
            },
            CommandSpec {
                name: "uninstall",
                description: "Remove a package",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "shell",
                description: "Spawn shell in virtualenv",
                relevance: 95.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "run",
                description: "Run command in virtualenv",
                relevance: 92.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "sync",
                description: "Install from Pipfile.lock",
                relevance: 85.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--dev"],
                    description: "Include dev packages",
                    relevance: 82.0,
                }],
            },
            CommandSpec {
                name: "lock",
                description: "Generate Pipfile.lock",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "graph",
                description: "Show dependency graph",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "check",
                description: "Check for security vulnerabilities",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[],
    },
    CommandSpec {
        name: "uv",
        description: "Extremely fast Python package/project manager (Astral)",
        relevance: 85.0,
        subcommands: &[
            CommandSpec {
                name: "pip install",
                description: "Install packages",
                relevance: 100.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["-r"],
                    description: "Install from requirements file",
                    relevance: 90.0,
                }],
            },
            CommandSpec {
                name: "pip sync",
                description: "Sync env to requirements",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "pip compile",
                description: "Compile requirements file",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "pip freeze",
                description: "List installed packages",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "venv",
                description: "Create a virtual environment",
                relevance: 92.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--python", "-p"],
                    description: "Python version to use",
                    relevance: 88.0,
                }],
            },
            CommandSpec {
                name: "run",
                description: "Run a command in the project env",
                relevance: 88.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "add",
                description: "Add a dependency to the project",
                relevance: 88.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--dev"],
                    description: "Add as dev dependency",
                    relevance: 82.0,
                }],
            },
            CommandSpec {
                name: "remove",
                description: "Remove a dependency",
                relevance: 82.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "sync",
                description: "Sync project dependencies",
                relevance: 90.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "lock",
                description: "Lock project dependencies",
                relevance: 85.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "init",
                description: "Initialize a new project",
                relevance: 80.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "tool install",
                description: "Install a tool globally",
                relevance: 78.0,
                subcommands: &[],
                flags: &[],
            },
            CommandSpec {
                name: "tool run",
                description: "Run a tool (like uvx)",
                relevance: 75.0,
                subcommands: &[],
                flags: &[],
            },
        ],
        flags: &[FlagSpec {
            names: &["--python", "-p"],
            description: "Python version to use",
            relevance: 80.0,
        }],
    },
    CommandSpec {
        name: "ruff",
        description: "Extremely fast Python linter and formatter",
        relevance: 82.0,
        subcommands: &[
            CommandSpec {
                name: "check",
                description: "Lint Python files",
                relevance: 100.0,
                subcommands: &[],
                flags: &[
                    FlagSpec {
                        names: &["--fix"],
                        description: "Auto-fix violations",
                        relevance: 92.0,
                    },
                    FlagSpec {
                        names: &["--watch"],
                        description: "Watch mode",
                        relevance: 82.0,
                    },
                    FlagSpec {
                        names: &["--select"],
                        description: "Rule codes to enable",
                        relevance: 75.0,
                    },
                ],
            },
            CommandSpec {
                name: "format",
                description: "Format Python files",
                relevance: 95.0,
                subcommands: &[],
                flags: &[FlagSpec {
                    names: &["--check"],
                    description: "Check without writing",
                    relevance: 88.0,
                }],
            },
        ],
        flags: &[
            FlagSpec {
                names: &["--config"],
                description: "Path to ruff.toml",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--line-length"],
                description: "Line length",
                relevance: 65.0,
            },
        ],
    },
    CommandSpec {
        name: "mypy",
        description: "Python static type checker",
        relevance: 78.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["--strict"],
                description: "Enable strict type checking",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["--ignore-missing-imports"],
                description: "Don't error on missing stubs",
                relevance: 82.0,
            },
            FlagSpec {
                names: &["--check-untyped-defs"],
                description: "Check untyped function bodies",
                relevance: 78.0,
            },
            FlagSpec {
                names: &["--disallow-untyped-defs"],
                description: "Require annotations on all functions",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["--no-implicit-optional"],
                description: "Disable implicit Optional",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["--show-error-codes"],
                description: "Show error codes in output",
                relevance: 72.0,
            },
            FlagSpec {
                names: &["--install-types"],
                description: "Auto-install missing stubs",
                relevance: 70.0,
            },
        ],
    },
    // ── Ruby ecosystem ────────────────────────────────────────────────────────
    CommandSpec {
        name: "ruby",
        description: "Ruby interpreter",
        relevance: 75.0,
        subcommands: &[],
        flags: &[
            FlagSpec {
                names: &["-e"],
                description: "Execute one-liner script",
                relevance: 88.0,
            },
            FlagSpec {
                names: &["-r"],
                description: "Require library before executing",
                relevance: 75.0,
            },
            FlagSpec {
                names: &["-w"],
                description: "Enable warnings",
                relevance: 68.0,
            },
            FlagSpec {
                names: &["-v"],
                description: "Print version",
                relevance: 65.0,
            },
        ],
    },
];
