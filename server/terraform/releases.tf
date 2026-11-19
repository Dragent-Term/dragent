# Desktop-app release hosting: private S3 bucket served through CloudFront.
# The release workflow (tag push v*) uploads DMGs/installers + stable/latest.json;
# the app's auto-updater and the download page read them via the CloudFront URL.
#
# URL layout:
#   https://<distribution-domain>/stable/latest.json
#   https://<distribution-domain>/stable/<version>/Dragent-<version>-macos-arm64.dmg
#   ...
# A custom domain (releases.dragent.dev) can be added later: ACM cert in
# us-east-1 + `aliases` on the distribution + a Cloudflare CNAME.

resource "aws_s3_bucket" "releases" {
  bucket = "${var.app_name}-releases-${data.aws_caller_identity.current.account_id}"
}

data "aws_caller_identity" "current" {}

resource "aws_s3_bucket_public_access_block" "releases" {
  bucket                  = aws_s3_bucket.releases.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# Only CloudFront (via OAC) may read the bucket.
resource "aws_s3_bucket_policy" "releases" {
  bucket = aws_s3_bucket.releases.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "AllowCloudFrontRead"
      Effect    = "Allow"
      Principal = { Service = "cloudfront.amazonaws.com" }
      Action    = "s3:GetObject"
      Resource  = "${aws_s3_bucket.releases.arn}/*"
      Condition = {
        StringEquals = { "AWS:SourceArn" = aws_cloudfront_distribution.releases.arn }
      }
    }]
  })
}

resource "aws_cloudfront_origin_access_control" "releases" {
  name                              = "${var.app_name}-releases"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

resource "aws_cloudfront_distribution" "releases" {
  enabled         = true
  comment         = "${var.app_name} desktop releases"
  is_ipv6_enabled = true
  price_class     = "PriceClass_100" # NA + EU edges; cheapest tier

  origin {
    domain_name              = aws_s3_bucket.releases.bucket_regional_domain_name
    origin_id                = "s3-releases"
    origin_access_control_id = aws_cloudfront_origin_access_control.releases.id
  }

  default_cache_behavior {
    target_origin_id       = "s3-releases"
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods        = ["GET", "HEAD"]
    cached_methods         = ["GET", "HEAD"]
    compress               = true
    # AWS managed "CachingOptimized" policy — long TTLs; artifacts are immutable
    # (versioned paths). latest.json is invalidated explicitly on publish.
    cache_policy_id = "658327ea-f89d-4fab-a63d-7e88639e58f6"
  }

  restrictions {
    geo_restriction { restriction_type = "none" }
  }

  viewer_certificate {
    cloudfront_default_certificate = true
  }
}

# ── GitHub Actions release role ───────────────────────────────────────────────
# Assumed ONLY by workflow runs for version tags (v1.2.3) on this repo. Can
# upload release artifacts and invalidate the manifest cache — nothing else.
resource "aws_iam_role" "github_actions_release" {
  name = "${var.app_name}-release-github-actions"
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
          "token.actions.githubusercontent.com:sub" = "repo:${var.github_repo}:ref:refs/tags/v*"
        }
      }
    }]
  })
}

resource "aws_iam_role_policy" "github_actions_release" {
  name = "${var.app_name}-gha-release-publish"
  role = aws_iam_role.github_actions_release.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "UploadReleaseArtifacts"
        Effect   = "Allow"
        Action   = ["s3:PutObject", "s3:GetObject", "s3:ListBucket"]
        Resource = [aws_s3_bucket.releases.arn, "${aws_s3_bucket.releases.arn}/*"]
      },
      {
        Sid      = "InvalidateManifest"
        Effect   = "Allow"
        Action   = "cloudfront:CreateInvalidation"
        Resource = aws_cloudfront_distribution.releases.arn
      },
    ]
  })
}

output "releases" {
  value = {
    bucket                  = aws_s3_bucket.releases.bucket
    cloudfront_domain       = aws_cloudfront_distribution.releases.domain_name
    cloudfront_distribution = aws_cloudfront_distribution.releases.id
    release_role_arn        = aws_iam_role.github_actions_release.arn
    manifest_url            = "https://${aws_cloudfront_distribution.releases.domain_name}/stable/latest.json"
  }
  description = "Desktop release hosting — the manifest_url is what the app's updater polls."
}
