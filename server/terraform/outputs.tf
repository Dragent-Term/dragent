output "ecr_repository_url" {
  value       = aws_ecr_repository.server.repository_url
  description = "Shared registry the EC2 deploy and CI both push to / pull from."
}

# Per-env CI deploy roles. Grab an env's ARN for its GitHub Environment secret:
#   terraform output -json deploy_role_arns | jq -r .production
output "deploy_role_arns" {
  value       = { for k, r in aws_iam_role.github_actions : k => r.arn }
  description = "Per-env GitHub Actions push-to-ECR role ARNs (set as each env's AWS_ROLE_ARN secret)."
}
