#!/usr/bin/env bash
# One-time migration: copy each config secret from AWS Secrets Manager
# (<app>/<env>/<KEY>) to SSM Parameter Store as a SecureString
# (/<app>/<env>/<KEY>). Values are read into a shell variable and written
# straight back to SSM — they are NEVER printed (tracing stays off).
#
# Run this BEFORE `terraform apply` of the websocket config: the EC2 boot script
# now reads from Parameter Store, so the parameters must already exist.
#
# It does NOT delete the Secrets Manager entries — verify the new instance boots
# from SSM first, then delete them separately if you want.
#
# Requirements: AWS credentials for the account that owns the secrets, and the
# aws CLI. Idempotent: re-running overwrites the SSM values from Secrets Manager.
#
# Usage:
#   AWS_REGION=us-east-1 ./migrate-secrets-to-ssm.sh            # migrate staging
#   APP_NAME=dragent ENV=production ./migrate-secrets-to-ssm.sh # another env
set -euo pipefail

REGION="${AWS_REGION:-us-east-1}"
APP_NAME="${APP_NAME:-dragent}"
ENV="${ENV:-staging}"

# The same keys the websocket config lists (server/terraform/websocket/main.tf).
KEYS=(
  DATABASE_URL JWT_SECRET REDIS_URL DRAGENT_WEB_ORIGIN
  OPENAI_API_KEY ANTHROPIC_API_KEY VOYAGE_API_KEY XAI_API_KEY GEMINI_API_KEY
  EXA_API_KEY
  GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET GOOGLE_CLIENT_ID GOOGLE_CLIENT_SECRET
  LEMONSQUEEZY_API_KEY LEMONSQUEEZY_STORE_ID LEMONSQUEEZY_WEBHOOK_SECRET
  SMTP_PASS DEEPGRAM_API_KEY DEEPGRAM_PROJECT_ID
)

echo "Migrating ${#KEYS[@]} secrets: Secrets Manager ($APP_NAME/$ENV/*) -> SSM (/$APP_NAME/$ENV/*) in $REGION"
migrated=0 skipped=0

for KEY in "${KEYS[@]}"; do
  SM_ID="$APP_NAME/$ENV/$KEY"
  SSM_NAME="/$APP_NAME/$ENV/$KEY"

  # Read the value from Secrets Manager. Missing/empty secrets are skipped, not
  # written (an EC2 boot fails on a blank env value, so don't create empties).
  if ! VAL="$(aws secretsmanager get-secret-value \
      --region "$REGION" --secret-id "$SM_ID" \
      --query SecretString --output text 2>/dev/null)"; then
    echo "  skip  $KEY  (not found in Secrets Manager)"
    skipped=$((skipped + 1))
    continue
  fi
  if [ -z "$VAL" ] || [ "$VAL" = "None" ]; then
    echo "  skip  $KEY  (empty in Secrets Manager)"
    skipped=$((skipped + 1))
    continue
  fi

  # Write to Parameter Store as an encrypted SecureString (AWS-managed SSM key).
  # --overwrite makes re-runs idempotent. Output is discarded (contains the name,
  # not the value, but keep it quiet anyway).
  aws ssm put-parameter \
    --region "$REGION" --name "$SSM_NAME" \
    --type SecureString --value "$VAL" --overwrite >/dev/null
  echo "  ok    $KEY  -> $SSM_NAME"
  migrated=$((migrated + 1))
  unset VAL
done

echo "Done: $migrated migrated, $skipped skipped."
echo "Next: terraform -chdir=server/terraform/websocket apply  (the instance re-boots reading from SSM)."
