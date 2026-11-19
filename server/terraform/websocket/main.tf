# Isolated config for the WebSocket-capable EC2 host (staging/beta).
#
# Kept in its OWN state, deliberately decoupled from the root config: the root's
# Terraform state was lost, so running it tries to recreate the live shared infra
# (ECR / secrets / …). This config manages ONLY the EC2 box. It does not create
# or read those shared resources — it derives the ECR image URL from
# account+region and reads the config values from SSM Parameter Store by name at
# boot (migrated off Secrets Manager — see scripts/migrate-secrets-to-ssm.sh).

terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.aws_region
}

provider "cloudflare" {
  # Uses CLOUDFLARE_API_TOKEN from the environment.
}

data "aws_caller_identity" "me" {}
data "aws_region" "me" {}

locals {
  # The image registry already exists; its URL/ARN are deterministic, so we build
  # them rather than calling ecr:DescribeRepositories (avoids another IAM grant).
  ecr_repository_url = "${data.aws_caller_identity.me.account_id}.dkr.ecr.${data.aws_region.me.name}.amazonaws.com/${var.app_name}-server"
  ecr_repository_arn = "arn:aws:ecr:${data.aws_region.me.name}:${data.aws_caller_identity.me.account_id}:repository/${var.app_name}-server"

  # The SAME config keys, now stored as SSM Parameter Store SecureStrings under
  # /<app_name>/staging/*. The instance reads their VALUES from Parameter Store at
  # boot (migrated from Secrets Manager — see scripts/migrate-secrets-to-ssm.sh).
  secret_names = [
    "DATABASE_URL", "JWT_SECRET", "REDIS_URL", "DRAGENT_WEB_ORIGIN",
    "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "VOYAGE_API_KEY", "XAI_API_KEY", "GEMINI_API_KEY",
    "EXA_API_KEY",
    "GITHUB_CLIENT_ID", "GITHUB_CLIENT_SECRET", "GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET",
    "LEMONSQUEEZY_API_KEY", "LEMONSQUEEZY_STORE_ID", "LEMONSQUEEZY_WEBHOOK_SECRET",
    "SMTP_PASS", "DEEPGRAM_API_KEY", "DEEPGRAM_PROJECT_ID",
    # The KMS key (alias) that seals the keys of services teams share
    # (Linear, Sentry, …). The key itself: alias/dragent-staging-integrations,
    # created by hand on 2026-11-17 with yearly rotation; never delete it,
    # saved connections can't be opened without it.
    "INTEGRATIONS_KMS_KEY_ID",
    # Apple push (phone notifications). APNS_PRIVATE_KEY is the .p8 file;
    # the server accepts it on one line with \n for its line breaks.
    "APNS_KEY_ID", "APNS_TEAM_ID", "APNS_BUNDLE_ID", "APNS_SANDBOX", "APNS_PRIVATE_KEY",
    # Android push (Firebase Cloud Messaging): the service account, one line.
    "FCM_PROJECT_ID", "FCM_SERVICE_ACCOUNT_JSON",
    # Shared with the Amplify auth portal: lets the API trust the visitor IP
    # the portal forwards, which turns on the per-IP sign-in rate limit.
    # Same value in SSM for each env and in the Amplify app (see README).
    "AUTH_PORTAL_PROXY_SECRET",
  ]

  # Plain (non-secret) SMTP settings — mirrors the App Runner module's Resend config.
  smtp_env = {
    SMTP_HOST   = "smtp.resend.com"
    SMTP_PORT   = "587"
    SMTP_USER   = "resend"
    SMTP_SENDER = "login@dragent.dev"
  }
}

module "staging_ec2" {
  source                   = "../modules/ec2-service"
  integrations_kms_key_arn = "arn:aws:kms:us-east-1:269235677023:key/082f1cf1-333b-438f-88d6-57de0dc23106"

  app_name       = var.app_name
  env            = "staging"
  domain         = var.staging_domain
  forward_domain = var.forward_domain
  acme_email     = var.acme_email
  # x86 for now: the CURRENT :staging image in ECR is amd64-only, so this runs it
  # immediately. Switch to "t4g.small" (arm64, ~$3/mo cheaper) after a staging
  # deploy has pushed the new multi-arch image. The module auto-selects the
  # matching AMI arch from this instance type.
  instance_type = "t4g.small"

  ecr_repository_url = local.ecr_repository_url
  ecr_repository_arn = local.ecr_repository_arn

  secret_names = local.secret_names
  smtp_env     = local.smtp_env

  cloudwatch_log_group_name     = "/dragent/${var.app_name}/staging/websocket"
  cloudwatch_log_retention_days = var.cloudwatch_log_retention_days
}

module "production_ec2" {
  count                    = var.enable_production_host ? 1 : 0
  source                   = "../modules/ec2-service"
  integrations_kms_key_arn = "arn:aws:kms:us-east-1:269235677023:key/082f1cf1-333b-438f-88d6-57de0dc23106"

  app_name       = var.app_name
  env            = "production"
  image_tag      = "production"
  domain         = var.production_domain
  forward_domain = ""
  acme_email     = var.acme_email
  instance_type  = "t4g.small"

  ecr_repository_url = local.ecr_repository_url
  ecr_repository_arn = local.ecr_repository_arn
  secret_names       = local.secret_names
  smtp_env           = local.smtp_env

  cloudwatch_log_group_name     = "/dragent/${var.app_name}/production/websocket"
  cloudwatch_log_retention_days = var.cloudwatch_log_retention_days
}

resource "cloudflare_dns_record" "forward_wildcard" {
  count   = var.cloudflare_zone_id == "" ? 0 : 1
  zone_id = var.cloudflare_zone_id
  name    = "*.${var.forward_domain}"
  type    = "A"
  # Cloudflare controls TTL for proxied records; Terraform requires 1 here.
  ttl     = 1
  content = module.staging_ec2.public_ip
  proxied = true
}

# Apply forwarding configuration to the existing hosts through SSM. This does
# not create or replace an EC2 instance: it installs the shared blue/green
# deploy script + Caddy template (byte-identical to what user_data bootstraps,
# via module outputs), lands the forwarding env value, re-renders Caddy against
# the active port, and runs one blue/green pass (which also migrates pre-
# blue/green hosts running the legacy `server` container). Any change to the
# deploy/template sources updates the rendered commands and SSM re-converges.
resource "aws_ssm_association" "forwarding_config" {
  name             = "AWS-RunShellScript"
  association_name = "${var.app_name}-staging-forwarding-config"

  targets {
    key    = "InstanceIds"
    values = [module.staging_ec2.instance_id]
  }

  parameters = {
    commands = templatefile("${path.module}/forwarding_update.sh.tftpl", {
      acme_email         = var.acme_email
      container_port     = module.staging_ec2.container_port
      domain             = var.staging_domain
      forward_domain     = var.forward_domain
      deploy_script      = module.staging_ec2.deploy_script
      caddyfile_template = module.staging_ec2.caddyfile_template
    })
  }
}

# Same convergence for the optional production host. Created only when the
# host itself is enabled; forwards stay off there (forward_domain = "").
resource "aws_ssm_association" "production_converge" {
  count            = var.enable_production_host ? 1 : 0
  name             = "AWS-RunShellScript"
  association_name = "${var.app_name}-production-host-converge"

  targets {
    key    = "InstanceIds"
    values = [module.production_ec2[0].instance_id]
  }

  parameters = {
    commands = templatefile("${path.module}/forwarding_update.sh.tftpl", {
      acme_email         = var.acme_email
      container_port     = module.production_ec2[0].container_port
      domain             = var.production_domain
      forward_domain     = ""
      deploy_script      = module.production_ec2[0].deploy_script
      caddyfile_template = module.production_ec2[0].caddyfile_template
    })
  }
}
