variable "aws_region" {
  type        = string
  default     = "us-east-1"
  description = "AWS region. us-east-1 sits closest to the LLM providers, which dominate agent-path latency."
}

variable "app_name" {
  type    = string
  default = "dragent"
}

variable "github_repo" {
  type = string
  # NOTE: lowercase `dragent` — this MUST be GitHub's canonical repo casing,
  # which is what the OIDC token's `sub` claim uses. IAM matches it
  # case-sensitively, so `Dragent` here silently breaks AssumeRoleWithWebIdentity
  # even though the git remote URL (case-insensitive) shows `Dragent`.
  default     = "Dragent-Term/dragent"
  description = "owner/repo (canonical GitHub casing). The deploy roles trust this repo's environment/branch OIDC subjects."
}

# NOTE: container_port / max_instances lived here for the removed App Runner
# module. The EC2 host owns its own port/instance config in ./websocket/.
