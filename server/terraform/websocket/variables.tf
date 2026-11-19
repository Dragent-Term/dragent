variable "aws_region" {
  type    = string
  default = "us-east-1"
}

variable "app_name" {
  type    = string
  default = "dragent"
}

variable "staging_domain" {
  type        = string
  default     = "beta.api.dragent.dev"
  description = "Hostname Caddy serves + gets a cert for. Point this A record at the module's public_ip after apply."
}

variable "enable_production_host" {
  type        = bool
  default     = false
  description = "Create a separate production WebSocket/API EC2 host when explicitly enabled."
}

variable "production_domain" {
  type        = string
  default     = "api.dragent.dev"
  description = "Hostname served by the optional production EC2 host."
}

variable "forward_domain" {
  type        = string
  default     = "dragent.dev"
  description = "Parent domain for hostname-based forwards (wildcard DNS must point to the EC2 Elastic IP)."
}

variable "cloudflare_zone_id" {
  type        = string
  default     = ""
  description = "Optional Cloudflare zone ID for managing *.forward_domain. Leave empty when DNS is managed elsewhere."
}

variable "acme_email" {
  type        = string
  default     = ""
  description = "Contact email for Let's Encrypt (optional)."
}

variable "cloudwatch_log_retention_days" {
  type        = number
  default     = 30
  description = "How many days to retain WebSocket EC2 logs in CloudWatch."
}
