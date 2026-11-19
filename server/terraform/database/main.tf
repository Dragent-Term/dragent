# Production Postgres (RDS) for the Dragent backend.
#
# Kept in its OWN state, like ./websocket and for the same reason: the root
# config's state was lost, so running it tries to recreate the live shared infra
# (ECR / OIDC / releases). This config manages ONLY the database.
#
# What it does NOT do: repoint the running API server. Terraform writes the new
# connection URL to a NEW SSM parameter (/<app>/production/DATABASE_URL) and
# leaves the live /<app>/staging/DATABASE_URL alone, so applying this is safe
# while the current server keeps serving. Cutover is a deliberate manual step —
# see README.md.

# ─────────────────────────────────────────────────────────────────────────────
#  DISABLED — production runs on Neon, not RDS.
#
#  Everything below is commented out on purpose. `terraform apply` in this
#  directory creates NOTHING while it stays that way, so the config cannot be
#  run by accident.
#
#  Why: at zero users, RDS costs ~$17.60/month (db.t4g.micro + 20 GB gp3 +
#  ~$3.65 for the public IPv4 that CI migrations need) for managed backups and
#  PITR that Neon's free tier already covers at $0. The economics flip once
#  there is revenue and the compute/storage caps start to bite — see
#  README.md.
#
#  To re-enable: delete the /* on the next line and the */ at the end of this
#  file, do the same in outputs.tf, then follow README.md (the IAM grants and
#  the migrations-reachability decision both still apply).
# ─────────────────────────────────────────────────────────────────────────────

/*
terraform {
  required_version = ">= 1.5"
  required_providers {
    aws    = { source = "hashicorp/aws", version = "~> 5.0" }
    random = { source = "hashicorp/random", version = "~> 3.6" }
  }
}

provider "aws" {
  region = var.aws_region
}

# ── Where the API host lives ──────────────────────────────────────────────────
# Looked up by Name tag rather than hardcoded ids: the EC2 host is recreated on
# every user_data change (user_data_replace_on_change = true), so a pinned
# instance id would go stale on the next websocket apply.
data "aws_instances" "app" {
  instance_tags        = { Name = "${var.app_name}-${var.app_env}-server" }
  instance_state_names = ["running"]
}

data "aws_instance" "app" {
  instance_id = one(data.aws_instances.app.ids)
}

# The aws_instance data source exposes subnet_id but not vpc_id, so the VPC is
# resolved through the subnet the host sits in.
data "aws_subnet" "app" {
  id = data.aws_instance.app.subnet_id
}

data "aws_subnets" "vpc" {
  filter {
    name   = "vpc-id"
    values = [data.aws_subnet.app.vpc_id]
  }
}

# ── Networking ────────────────────────────────────────────────────────────────
# A subnet group needs at least two AZs even for a single-AZ instance; the
# default VPC supplies one subnet per AZ, so passing them all satisfies that and
# leaves room to flip multi_az on later without re-plumbing.
resource "aws_db_subnet_group" "main" {
  name       = "${var.app_name}-${var.env}"
  subnet_ids = data.aws_subnets.vpc.ids

  tags = { Name = "${var.app_name}-${var.env}", Env = var.env }
}

resource "aws_security_group" "db" {
  name        = "${var.app_name}-${var.env}-db"
  description = "Postgres access for the ${var.app_name} ${var.env} API host"
  vpc_id      = data.aws_subnet.app.vpc_id

  tags = { Name = "${var.app_name}-${var.env}-db", Env = var.env }
}

# Source is the API host's OWN security group, not a CIDR: the host keeps its
# security group across replacements but not its private IP, so a CIDR rule
# would silently stop matching after the next instance swap.
resource "aws_vpc_security_group_ingress_rule" "from_app" {
  security_group_id            = aws_security_group.db.id
  referenced_security_group_id = one(data.aws_instance.app.vpc_security_group_ids)
  from_port                    = 5432
  to_port                      = 5432
  ip_protocol                  = "tcp"
  description                  = "Postgres from the API host"
}

# Neon-style open endpoint. Guarded by its own variable so it can never be
# switched on by accident, and by a precondition so it cannot be switched on
# without also giving the instance a public endpoint (which would otherwise
# produce a rule that silently does nothing).
resource "aws_vpc_security_group_ingress_rule" "public" {
  count = var.allow_public_ingress ? 1 : 0

  security_group_id = aws_security_group.db.id
  cidr_ipv4         = "0.0.0.0/0"
  from_port         = 5432
  to_port           = 5432
  ip_protocol       = "tcp"
  description       = "Public Postgres (TLS + password only) — see allow_public_ingress"

  lifecycle {
    precondition {
      condition     = var.publicly_accessible
      error_message = "allow_public_ingress needs publicly_accessible = true, or the rule opens a port nothing can route to."
    }
  }
}

# Optional break-glass / migration access from named CIDRs. Empty by default —
# see README.md for why CI migrations need thought here.
resource "aws_vpc_security_group_ingress_rule" "extra" {
  for_each = { for cidr in var.extra_ingress_cidrs : cidr => cidr }

  security_group_id = aws_security_group.db.id
  cidr_ipv4         = each.value
  from_port         = 5432
  to_port           = 5432
  ip_protocol       = "tcp"
  description       = "Operator / CI access"
}

# ── Credentials ───────────────────────────────────────────────────────────────
# No special characters, deliberately. The password is interpolated into a
# postgres:// URL, and a `/`, `@`, `?` or `#` in there needs percent-encoding —
# a mangled connection URL has already caused one total outage in this project
# (see app/src/api/backend.rs). 32 alphanumerics is ~190 bits; the entropy loss
# is irrelevant next to the failure mode it removes.
resource "random_password" "master" {
  length  = 32
  special = false
}

# ── The instance ──────────────────────────────────────────────────────────────
resource "aws_db_instance" "main" {
  identifier = "${var.app_name}-${var.env}"

  engine = "postgres"
  # Major version only: RDS selects the current minor, so an apply cannot fail
  # on a minor that was retired between writing this and running it.
  engine_version             = var.engine_version
  auto_minor_version_upgrade = true

  instance_class        = var.instance_class
  allocated_storage     = var.allocated_storage
  max_allocated_storage = var.max_allocated_storage # 0 disables autoscaling
  storage_type          = "gp3"
  storage_encrypted     = true

  db_name  = var.db_name
  username = var.master_username
  password = random_password.master.result
  port     = 5432

  db_subnet_group_name   = aws_db_subnet_group.main.name
  vpc_security_group_ids = [aws_security_group.db.id]
  publicly_accessible    = var.publicly_accessible
  multi_az               = var.multi_az

  backup_retention_period = var.backup_retention_days
  backup_window           = var.backup_window
  maintenance_window      = var.maintenance_window
  copy_tags_to_snapshot   = true

  # Guardrails for a database holding real customer + billing records. Flipping
  # either of these is a conscious decision, not a side effect of `destroy`.
  deletion_protection       = var.deletion_protection
  skip_final_snapshot       = false
  final_snapshot_identifier = "${var.app_name}-${var.env}-final-${formatdate("YYYYMMDDhhmm", timestamp())}"

  performance_insights_enabled = var.performance_insights
  apply_immediately            = false

  tags = { Name = "${var.app_name}-${var.env}", Env = var.env }

  lifecycle {
    # `timestamp()` above changes on every plan; without this the instance would
    # show a spurious diff (and a replacement) on each run.
    ignore_changes = [final_snapshot_identifier]
  }
}

# ── Connection URL → SSM ──────────────────────────────────────────────────────
# Written by Terraform rather than pasted by a human, for the same reason the
# password avoids special characters: this exact string has been mangled by
# hand before. sslmode=require because the traffic crosses the VPC.
locals {
  database_url = format(
    "postgresql://%s:%s@%s:%d/%s?sslmode=require",
    var.master_username,
    random_password.master.result,
    aws_db_instance.main.address,
    aws_db_instance.main.port,
    var.db_name,
  )
}

resource "aws_ssm_parameter" "database_url" {
  name        = var.ssm_database_url_name
  description = "Postgres connection URL for ${var.app_name} ${var.env}"
  type        = "SecureString"
  value       = local.database_url
  overwrite   = true

  tags = { Env = var.env }
}
*/
