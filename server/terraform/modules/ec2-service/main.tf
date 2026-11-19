# EC2 + Caddy host for the Go server.
#
# WHY THIS EXISTS: App Runner does not support WebSockets — it rejects the
# `Upgrade: websocket` handshake at ingress (403) before it reaches the
# container, so the shared-session gateway (/realtime) can never run there. A
# plain VM speaks WebSockets natively; Caddy fronts it with auto-TLS. One small
# instance serves BOTH the REST/GraphQL API and the realtime socket.
#
# The instance is stateless: Postgres (Neon) and Redis (Upstash) are external
# managed services reached over public TLS, so there's no VPC/RDS/ElastiCache to
# peer with. Secrets are read at boot from SSM Parameter Store SecureStrings under
# /<app_name>/<env>/*, so credentials aren't duplicated in the image or Terraform.

locals {
  name      = "${var.app_name}-${var.env}"
  image_tag = var.image_tag != "" ? var.image_tag : var.env
  # Graviton (arm64) families: a1, and c/m/r/t<gen>g. Anything else → x86_64.
  is_arm   = length(regexall("^(a1|[cmrt][0-9]+g)", var.instance_type)) > 0
  ami_arch = local.is_arm ? "arm64" : "x86_64"

  # Single source of truth for the host's deploy path. user_data bootstraps new
  # hosts with these files and the websocket SSM associations re-deliver them
  # to live hosts (user_data is ignored after first boot), so both stay equal.
  deploy_vars = {
    region         = data.aws_region.current.name
    image          = "${var.ecr_repository_url}:${local.image_tag}"
    container_port = var.container_port
    secret_prefix  = "/${var.app_name}/${var.env}"
    secret_keys    = join(" ", var.secret_names)
    plain_env      = join("\n", concat(["PORT=${var.container_port}", "APP_ENV=${var.env}", "DRAGENT_FORWARD_DOMAIN=${var.forward_domain}"], [for k, v in var.smtp_env : "${k}=${v}"]))
  }
  caddy_vars = {
    domain         = var.domain
    acme_email     = var.acme_email
    forward_domain = var.forward_domain
  }
  # Whole-line comments are dropped from what ships: the script rides inside
  # user_data, which EC2 caps at 16 KB. The source keeps them.
  deploy_script      = replace(templatefile("${path.module}/deploy.sh.tftpl", local.deploy_vars), "/(?m)^[ \\t]*#[^!\\n].*\\n/", "")
  caddyfile_template = templatefile("${path.module}/Caddyfile.tftpl", local.caddy_vars)
}

data "aws_region" "current" {}
data "aws_caller_identity" "current" {}

# Latest Amazon Linux 2023 AMI for the instance's architecture. Resolved via
# DescribeImages (ec2:*) rather than an SSM public parameter, so the caller
# needs only EC2 permissions — no extra ssm:GetParameter grant.
data "aws_ami" "al2023" {
  most_recent = true
  owners      = ["amazon"]
  filter {
    name   = "name"
    values = ["al2023-ami-2023.*-${local.ami_arch}"]
  }
  filter {
    name   = "architecture"
    values = [local.ami_arch]
  }
  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

# ── Networking ────────────────────────────────────────────────────────────────
# Uses the account's default VPC/subnet — the box only needs public inbound 80/
# 443 and public outbound (to Neon, Upstash, ECR, LLM providers, Let's Encrypt).
data "aws_vpc" "default" {
  default = true
}

resource "aws_security_group" "server" {
  name        = "${local.name}-server"
  description = "HTTP/HTTPS in (Caddy + WebSockets); all out."
  vpc_id      = data.aws_vpc.default.id

  ingress {
    description      = "HTTP (ACME challenge + redirect to HTTPS)"
    from_port        = 80
    to_port          = 80
    protocol         = "tcp"
    cidr_blocks      = ["0.0.0.0/0"]
    ipv6_cidr_blocks = ["::/0"]
  }
  ingress {
    description      = "HTTPS + WebSocket upgrade"
    from_port        = 443
    to_port          = 443
    protocol         = "tcp"
    cidr_blocks      = ["0.0.0.0/0"]
    ipv6_cidr_blocks = ["::/0"]
  }
  dynamic "ingress" {
    for_each = var.ssh_ingress_cidr == "" ? [] : [var.ssh_ingress_cidr]
    content {
      description = "SSH (optional; prefer SSM Session Manager)"
      from_port   = 22
      to_port     = 22
      protocol    = "tcp"
      cidr_blocks = [ingress.value]
    }
  }

  egress {
    description      = "All outbound (DBs, ECR, LLM providers, ACME)"
    from_port        = 0
    to_port          = 0
    protocol         = "-1"
    cidr_blocks      = ["0.0.0.0/0"]
    ipv6_cidr_blocks = ["::/0"]
  }

  tags = { Name = "${local.name}-server" }
}

# ── IAM: instance role ────────────────────────────────────────────────────────
resource "aws_iam_role" "instance" {
  name = "${local.name}-ec2-instance"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
    }]
  })
}

# Read ONLY this env's secrets from SSM Parameter Store (SecureString), scoped to
# the env's parameter namespace. Reading a SecureString needs BOTH ssm:GetParameter
# AND kms:Decrypt — the latter is bounded to the AWS-managed SSM key via the
# `kms:ViaService` condition, so the role can decrypt ONLY through SSM, never by
# calling KMS directly.
resource "aws_iam_role_policy" "secrets" {
  name = "${local.name}-read-params"
  role = aws_iam_role.instance.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = ["ssm:GetParameter", "ssm:GetParameters"]
        Resource = "arn:aws:ssm:${data.aws_region.current.name}:${data.aws_caller_identity.current.account_id}:parameter/${var.app_name}/${var.env}/*"
      },
      {
        Effect   = "Allow"
        Action   = ["kms:Decrypt"]
        Resource = "*"
        Condition = {
          StringEquals = {
            "kms:ViaService" = "ssm.${data.aws_region.current.name}.amazonaws.com"
          }
        }
      },
    ]
  })
}

# Seal and open the keys of connected services with their own KMS key, and
# only for that purpose (the encryption context the server binds in).
resource "aws_iam_role_policy" "integrations_kms" {
  count = var.integrations_kms_key_arn == "" ? 0 : 1
  name  = "${local.name}-integrations-kms"
  role  = aws_iam_role.instance.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid      = "SealIntegrationKeys"
      Effect   = "Allow"
      Action   = ["kms:GenerateDataKey", "kms:Decrypt"]
      Resource = var.integrations_kms_key_arn
      Condition = {
        StringEquals = {
          "kms:EncryptionContext:app"     = "dragent"
          "kms:EncryptionContext:purpose" = "integration-credentials"
        }
      }
    }]
  })
}

# Pull the image from ECR (auth token is account-scoped; layer reads are repo-scoped).
resource "aws_iam_role_policy" "ecr_pull" {
  name = "${local.name}-ecr-pull"
  role = aws_iam_role.instance.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = "ecr:GetAuthorizationToken"
        Resource = "*"
      },
      {
        Effect = "Allow"
        Action = [
          "ecr:BatchCheckLayerAvailability",
          "ecr:GetDownloadUrlForLayer",
          "ecr:BatchGetImage",
        ]
        Resource = var.ecr_repository_arn
      },
    ]
  })
}

# The CloudWatch agent only needs to create streams and publish events into
# this module's log group. DescribeLogStreams is required when it resumes an
# existing stream after an instance restart.
resource "aws_iam_role_policy" "cloudwatch_logs" {
  name = "${local.name}-cloudwatch-logs"
  role = aws_iam_role.instance.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "logs:CreateLogStream",
        "logs:DescribeLogStreams",
        "logs:PutLogEvents",
      ]
      Resource = "${aws_cloudwatch_log_group.server.arn}:*"
    }]
  })
}

# Session Manager shell access — no SSH keys, no open port 22.
resource "aws_iam_role_policy_attachment" "ssm" {
  role       = aws_iam_role.instance.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_instance_profile" "instance" {
  name = "${local.name}-ec2-instance"
  role = aws_iam_role.instance.name
}

# ── Logging ────────────────────────────────────────────────────────────────────
resource "aws_cloudwatch_log_group" "server" {
  name              = var.cloudwatch_log_group_name
  retention_in_days = var.cloudwatch_log_retention_days

  tags = { Name = var.cloudwatch_log_group_name }
}

# ── Instance ──────────────────────────────────────────────────────────────────
resource "aws_instance" "server" {
  ami                    = data.aws_ami.al2023.id
  instance_type          = var.instance_type
  vpc_security_group_ids = [aws_security_group.server.id]
  iam_instance_profile   = aws_iam_instance_profile.instance.name

  metadata_options {
    http_tokens   = "required" # IMDSv2 only
    http_endpoint = "enabled"
  }

  root_block_device {
    volume_size = var.root_volume_gb
    volume_type = "gp3"
    encrypted   = true
  }

  user_data = templatefile("${path.module}/user_data.sh.tftpl", {
    region               = data.aws_region.current.name
    image                = "${var.ecr_repository_url}:${local.image_tag}"
    domain               = var.domain
    acme_email           = var.acme_email
    container_port       = var.container_port
    secret_prefix        = "/${var.app_name}/${var.env}"
    secret_keys          = join(" ", var.secret_names)
    cloudwatch_log_group = var.cloudwatch_log_group_name
    forward_domain       = var.forward_domain
    plain_env            = join("\n", concat(["PORT=${var.container_port}", "APP_ENV=${var.env}", "DRAGENT_FORWARD_DOMAIN=${var.forward_domain}"], [for k, v in var.smtp_env : "${k}=${v}"]))
    deploy_script        = local.deploy_script
    caddyfile_template   = local.caddyfile_template
  })
  # Configuration changes are applied in place by the deployment/SSM workflow.
  # Do not replace the live EC2 host when Caddy or forwarding settings change.
  user_data_replace_on_change = false

  # This host is managed in place. AMI refreshes and bootstrap template edits
  # must not destroy the live instance; deploy/SSM handles application changes.
  lifecycle {
    ignore_changes = [ami, user_data, user_data_replace_on_change]
  }

  tags = { Name = "${local.name}-server" }
}

# Static IP so the DNS A record stays valid across reboots/replacements.
resource "aws_eip" "server" {
  domain   = "vpc"
  instance = aws_instance.server.id
  tags     = { Name = "${local.name}-server" }
}
