variable "app_name" {
  type        = string
  description = "Application name, e.g. \"dragent\". Used to name resources and the secret namespace."
}

variable "env" {
  type        = string
  description = "Environment name, e.g. \"staging\" / \"production\". Secrets are read from <app_name>/<env>/*."
}

variable "domain" {
  type        = string
  description = "Public hostname this instance serves (e.g. beta.api.dragent.dev). Caddy obtains a Let's Encrypt cert for it, so DNS must point an A record at the instance's Elastic IP BEFORE the cert can issue."
}

variable "forward_domain" {
  type        = string
  default     = ""
  description = "Optional parent domain for hostname-based forwards, e.g. dragent.dev."
}

variable "acme_email" {
  type        = string
  default     = ""
  description = "Contact email for Let's Encrypt (expiry notices). Optional but recommended."
}

variable "ecr_repository_url" {
  type        = string
  description = "ECR repo URL the server image lives in (shared with the App Runner module)."
}

variable "ecr_repository_arn" {
  type        = string
  description = "ARN of the ECR repo, for the instance's pull policy."
}

variable "image_tag" {
  type        = string
  default     = ""
  description = "Image tag to run. Defaults to the env name (the moving tag CI pushes)."
}

variable "container_port" {
  type        = number
  default     = 8080
  description = "Port the server container listens on. Caddy reverse-proxies to it on localhost."
}

variable "instance_type" {
  type        = string
  default     = "t4g.small"
  description = "EC2 type. t4g.* is arm64/Graviton (cheapest); requires the arm64 image (multi-arch build). Use t3.* for amd64-only images."
}

variable "root_volume_gb" {
  type        = number
  default     = 12
  description = "Root EBS size. The server image + Docker + logs are small."
}

variable "secret_names" {
  type        = list(string)
  description = "Config keys to fetch from Secrets Manager (<app_name>/<env>/<key>) into the container's env at boot. MUST match the keys the App Runner module created, so the same secret values are reused."
}

variable "smtp_env" {
  type        = map(string)
  default     = {}
  description = "Plain (non-secret) env vars to inject, e.g. SMTP_HOST/PORT/USER/SENDER. Mirrors the App Runner module's smtp_env."
}

variable "ssh_ingress_cidr" {
  type        = string
  default     = ""
  description = "If set (e.g. \"1.2.3.4/32\"), opens port 22 to that CIDR. Left empty by default — shell access is via SSM Session Manager, so no SSH port is exposed."
}

variable "cloudwatch_log_group_name" {
  type        = string
  description = "CloudWatch Logs group receiving bootstrap and Docker container logs."
}

variable "cloudwatch_log_retention_days" {
  type        = number
  default     = 30
  description = "How many days CloudWatch retains WebSocket host logs."
  validation {
    condition     = contains([1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653], var.cloudwatch_log_retention_days)
    error_message = "cloudwatch_log_retention_days must be a supported CloudWatch Logs retention value."
  }
}

variable "integrations_kms_key_arn" {
  description = "KMS key that seals the keys of connected services; the server may use it only with Dragent's encryption context. Empty = no access."
  type        = string
  default     = ""
}
