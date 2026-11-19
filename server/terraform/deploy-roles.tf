# ── GitHub Actions deploy roles (CI push-to-ECR) ──────────────────────────────
# Extracted from the former App Runner module (modules/service, removed) so CI
# keeps a code-of-record now that App Runner is gone — the live server runs on
# the EC2 host in ./websocket/. Each env's deploy workflow assumes its role via
# the AWS_ROLE_ARN GitHub Environment secret, authenticates to ECR, and pushes
# the shared server image; the EC2 host polls ECR and redeploys. The trust
# policy accepts EITHER the GitHub Environment subject OR the branch subject —
# both mean "<env> from this repo". Scoped to this repo + env; nothing else.
locals {
  # env => branch that deploys it.
  deploy_envs = {
    production = "main"
    staging    = "staging"
  }
}

resource "aws_iam_role" "github_actions" {
  for_each = local.deploy_envs
  name     = "${var.app_name}-${each.key}-github-actions"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Federated = aws_iam_openid_connect_provider.github.arn }
      Action    = "sts:AssumeRoleWithWebIdentity"
      Condition = {
        StringEquals = {
          "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
        }
        StringLike = {
          "token.actions.githubusercontent.com:sub" = [
            "repo:${var.github_repo}:environment:${each.key}",
            "repo:${var.github_repo}:ref:refs/heads/${each.value}",
          ]
        }
      }
    }]
  })
}

# Least privilege: authenticate to ECR + push to the shared repo. Nothing else.
resource "aws_iam_role_policy" "github_actions_ecr" {
  for_each = local.deploy_envs
  name     = "${var.app_name}-${each.key}-gha-ecr-push"
  role     = aws_iam_role.github_actions[each.key].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "EcrAuthToken"
        Effect   = "Allow"
        Action   = "ecr:GetAuthorizationToken"
        Resource = "*" # account-scoped action; takes no resource ARN
      },
      {
        Sid    = "EcrPushToServerRepo"
        Effect = "Allow"
        Action = [
          "ecr:BatchCheckLayerAvailability",
          "ecr:InitiateLayerUpload",
          "ecr:UploadLayerPart",
          "ecr:CompleteLayerUpload",
          "ecr:PutImage",
          "ecr:BatchGetImage",
        ]
        Resource = aws_ecr_repository.server.arn
      },
    ]
  })
}
