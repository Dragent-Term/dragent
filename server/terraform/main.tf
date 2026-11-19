# Shared, account-wide infrastructure for Dragent's backend + desktop releases.
#
# NOTE: The live API server runs on a WebSocket-capable EC2 host defined in its
# OWN config + state under ./websocket/ (App Runner couldn't do WebSockets and
# has been removed). This root config now manages only the SHARED resources:
#   - the ECR image repo the EC2 deploy and CI both use   (ecr.tf)
#   - the account-wide GitHub Actions OIDC provider        (oidc.tf)
#   - the per-env CI push-to-ECR deploy roles              (deploy-roles.tf)
#   - desktop-app release hosting (S3 + CloudFront)        (releases.tf)
#
# ⚠ The root state was lost once (see ./websocket/main.tf); treat these files as
# the code-of-record for already-live resources and `terraform import` before
# any apply, or you'll get "already exists" errors.
terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
  # Recommended: store state in S3 + DynamoDB lock (uncomment and set your bucket).
  # backend "s3" {
  #   bucket         = "dragent-tfstate"
  #   key            = "prod/terraform.tfstate"
  #   region         = "us-east-1"
  #   dynamodb_table = "dragent-tflock"
  #   encrypt        = true
  # }
}

provider "aws" {
  region = var.aws_region
}
