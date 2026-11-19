variable "aws_region" {
  type    = string
  default = "us-east-1"
}

variable "app_name" {
  type    = string
  default = "dragent"
}

variable "env" {
  type        = string
  default     = "production"
  description = "Environment this database serves. Used in resource names and the SSM path."
}

variable "app_env" {
  type        = string
  default     = "staging"
  description = <<-EOT
    Env segment of the API host's Name tag, used to find the instance this
    database must be reachable from. It is "staging" even for the production
    database: there is one EC2 host, still named dragent-staging-server, and it
    is the box being cut over. Change this only if a separate host is stood up.
  EOT
}

# ── Sizing ────────────────────────────────────────────────────────────────────

variable "instance_class" {
  type        = string
  default     = "db.t4g.micro"
  description = <<-EOT
    Smallest current-generation Graviton class (2 vCPU burst, 1 GiB). Right for
    a service with no users yet; scaling up later is an in-place modify with a
    short restart, so starting small costs nothing but a maintenance window.
  EOT
}

variable "allocated_storage" {
  type        = number
  default     = 20
  description = "GiB. RDS minimum for gp3 is 20."
}

variable "max_allocated_storage" {
  type        = number
  default     = 100
  description = "Storage autoscaling ceiling in GiB. 0 disables autoscaling."
}

variable "engine_version" {
  type        = string
  default     = "17"
  description = <<-EOT
    Postgres MAJOR version. Deliberately not pinned to a minor so an apply
    cannot fail on a minor RDS has since retired. Confirm the major is offered
    in your region with:
      aws rds describe-db-engine-versions --engine postgres \
        --query 'DBEngineVersions[].EngineVersion'
    (that call needs rds:DescribeDBEngineVersions, which the current IAM user
    does not have — see README.md.)
  EOT
}

variable "multi_az" {
  type        = bool
  default     = false
  description = "Standby in a second AZ. Roughly doubles cost; off until there is traffic worth protecting."
}

# ── Access ────────────────────────────────────────────────────────────────────

variable "publicly_accessible" {
  type        = bool
  default     = false
  description = <<-EOT
    Whether the instance gets a public endpoint. Default false: reachable only
    from the API host's security group.

    This has a consequence — the deploy workflow runs `goose up` from a GitHub
    Actions runner, which cannot reach a private instance. Read the migrations
    section of README.md before flipping this to true; running migrations from
    the EC2 host is the better answer.
  EOT
}

variable "allow_public_ingress" {
  type        = bool
  default     = false
  description = <<-EOT
    Open 5432 to the internet, giving a connection URL that works from anywhere
    — CI, a laptop, the EC2 host — exactly like the Neon database this replaces.

    This is a deliberate, named opt-in rather than something reachable through
    extra_ingress_cidrs, because it is a real decision: the only thing standing
    between the internet and a database of customer and billing records is TLS
    plus the master password.

    Worth knowing before judging it: the Neon instance in use today is ALSO
    publicly reachable, so this is parity with the current posture, not a
    regression. Requires publicly_accessible = true.
  EOT
}

variable "extra_ingress_cidrs" {
  type        = list(string)
  default     = []
  description = <<-EOT
    Additional CIDRs allowed on 5432 (an office IP for psql, say). Only takes
    effect together with publicly_accessible = true. Never 0.0.0.0/0.
  EOT

  validation {
    condition     = !contains(var.extra_ingress_cidrs, "0.0.0.0/0")
    error_message = "Refusing 0.0.0.0/0: that exposes the production database to the entire internet."
  }
}

# ── Database ──────────────────────────────────────────────────────────────────

variable "db_name" {
  type    = string
  default = "dragent"
}

variable "master_username" {
  type    = string
  default = "dragent"
}

# ── Operations ────────────────────────────────────────────────────────────────

variable "backup_retention_days" {
  type        = number
  default     = 7
  description = "Automated backup retention. 0 disables backups — never do that here."

  validation {
    condition     = var.backup_retention_days >= 1
    error_message = "A database holding customer and billing records must keep backups."
  }
}

variable "backup_window" {
  type        = string
  default     = "07:00-08:00"
  description = "UTC. Off-peak for a US-centric audience."
}

variable "maintenance_window" {
  type    = string
  default = "sun:08:30-sun:09:30"
}

variable "deletion_protection" {
  type        = bool
  default     = true
  description = "Blocks `terraform destroy` and console deletion until explicitly turned off."
}

variable "performance_insights" {
  type        = bool
  default     = false
  description = "Free tier is 7 days of retention, but unsupported on some micro classes — leave off until needed."
}

variable "ssm_database_url_name" {
  type        = string
  default     = "/dragent/production/DATABASE_URL"
  description = <<-EOT
    Where the connection URL is written. Deliberately a NEW path: the live
    server reads /dragent/staging/DATABASE_URL, so applying this cannot disturb
    what is currently serving. Cutover is a separate, deliberate step.
  EOT
}
