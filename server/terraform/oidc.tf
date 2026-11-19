# Account-wide GitHub Actions OIDC provider — shared by BOTH environments' deploy
# roles (which live in deploy-roles.tf). GitHub mints a short-lived OIDC token
# per run; AWS trades it for temp creds. No long-lived AWS keys in GitHub.
resource "aws_iam_openid_connect_provider" "github" {
  url            = "https://token.actions.githubusercontent.com"
  client_id_list = ["sts.amazonaws.com"]
  # AWS no longer verifies the well-known GitHub IdP against this thumbprint (it
  # uses its own trust store), but the API still requires the field. GitHub's
  # published root CA thumbprint.
  thumbprint_list = ["6938fd4d98bab03faadb97b34396831e3780aea1"]
}
