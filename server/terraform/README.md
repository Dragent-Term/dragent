# Infrastructure (AWS · EC2 + shared services)

Right-sized AWS for a solo/pre-launch deployment. **Review and adjust before
`apply` — this provisions billable resources.**

## Layout

This directory holds **two independent Terraform configs with separate state**:

- **`./` (root)** — shared, account-wide resources only:
  - **ECR** repo for the server image (`ecr.tf`).
  - **GitHub Actions OIDC provider** (`oidc.tf`) + per-env **CI push-to-ECR
    deploy roles** (`deploy-roles.tf`) — the `AWS_ROLE_ARN` each env's deploy
    workflow assumes. No long-lived AWS keys in GitHub.
  - **Desktop-app release hosting**: private S3 bucket behind CloudFront, plus
    the tag-scoped release role the updater publishes through (`releases.tf`).

  > ⚠ The root state was lost once; treat these files as the code-of-record for
  > already-live resources and `terraform import` before any apply, or you'll
  > hit "already exists" errors.

- **`./websocket/`** — the live **API server**, on a WebSocket-capable **EC2 +
  Caddy** host (staging/beta, with an optional separate production host). Its own state, decoupled from root: it derives
  the ECR image URL from account+region and reads environment-scoped SSM
  Parameter Store values by name at boot. This is where the running server lives.

App Runner was removed: it can't proxy WebSockets, which the `/realtime` shared-
session gateway needs, so the server moved to EC2 (`./websocket/` +
`modules/ec2-service`).

## Deploy flow

CI (`.github/workflows/deploy.yml`) builds the multi-arch image, assumes the
env's deploy role, and pushes `:<env>` + `:<sha>` tags to the shared ECR repo.
The EC2 host polls ECR and redeploys within ~60s. It starts each replacement on
the idle localhost port (8080/8081), waits up to 90 seconds for `/ready`, reloads
Caddy to route new connections to it, then SIGTERMs the old container with
`docker stop -t 60` so it drains: the server stops reporting ready immediately,
waits 5 s, then gives in-flight requests up to 25 s to finish
(`readinessDrainGrace` / `shutdownGrace` in `server/cmd/api/main.go`).
WebSocket clients reconnect on their own; SSE streams longer than about 30 s
are still cut at the switch even with the 60 s stop timeout — the old process
is drained, not kept indefinitely.

The deploy script and Caddy template live in `modules/ec2-service/`
(`deploy.sh.tftpl`, `Caddyfile.tftpl`). New hosts bootstrap from them via
`user_data`; the instance ignores `user_data` after first boot
(`user_data_replace_on_change = false`, `ignore_changes`), so the same files
are re-delivered to live hosts through the `websocket/` SSM associations, which
re-converge the host whenever the sources change. Hosts still running the
pre-blue/green single `server` container are migrated in place: the next
converge pass treats it as the old generation on 8080, starts the replacement
on 8081, switches Caddy after `/ready`, then stops and removes the legacy
container.

## Optional production host

The websocket config defaults to staging only. To create a separate production
host, set `enable_production_host = true` and `production_domain` in the
`websocket/` Terraform variables (for example, via `terraform.tfvars`), confirm
that DNS for that hostname points to the resulting `production_ec2` public IP,
and apply the websocket config. The instance reads `/<app_name>/production/*`
from SSM Parameter Store and tracks the `:production` image tag. Populate the
required production parameters and DNS before enabling it; leaving the switch
false creates no production resources.

## Apply

```
# Shared services (rarely — import first, state was lost):
terraform init && terraform plan && terraform apply

# The live server (EC2):
cd websocket && terraform init && terraform plan && terraform apply
```

The WebSocket state also manages hostname-based browser forwards on the
existing EC2 host. Set `cloudflare_zone_id` to the Cloudflare zone ID for
`dragent.dev` and export `CLOUDFLARE_API_TOKEN` with DNS Read + DNS Write
permissions to create the proxied `*.dragent.dev` record. Leave the variable
empty when DNS is managed elsewhere and create the wildcard A record there,
pointing to the Terraform `public_ip` output. The forwarding configuration is
applied to the existing instance through SSM; it does not create another EC2
instance.

> ⚠ **`forward_domain` must be its own registrable domain** (e.g.
> `dragent-preview.dev`), **not `dragent.dev`** or any subdomain of the
> portal/API site. Forwards serve arbitrary user-controlled scripts; on a
> subdomain of `dragent.dev` they are same-site with the portal and API and can
> set cookies for it. The server reads it as `DRAGENT_FORWARD_DOMAIN`; set the
> same value as `DRAGENT_FORWARD_DOMAIN` (or `NEXT_PUBLIC_FORWARD_DOMAIN`) on the
> auth portal so its `/forward/...` links redirect there instead of using the
> sandboxed same-origin fallback proxy. Changing it means a new Cloudflare zone
> (and `cloudflare_zone_id`) for that domain.

In Cloudflare, keep the record proxied, set SSL/TLS mode to **Full (strict)**,
and ensure **WebSockets** is enabled. Cloudflare supplies the public edge
certificate for the wildcard hostname; Caddy handles the origin HTTPS
connection and passes WebSocket upgrades through to the API container.

## Secrets

Config keys live in SSM Parameter Store as SecureStrings under
`/<app_name>/<env>/<KEY>` (set out of band with `aws ssm put-parameter`). The
EC2 host reads them by name at boot and refreshes them during redeploy; the list
lives in `websocket/main.tf` (`local.secret_names`).

### `AUTH_PORTAL_PROXY_SECRET`

Shared between the API and the auth portal (Amplify). With it, the API trusts
the visitor IP the portal forwards on sign-in requests, which turns on the
per-IP sign-in rate limit. Until it is set on both sides the per-IP limit stays
off (the per-email limit still applies; the API logs a warning at start).

1. Generate a long random value, e.g. `openssl rand -hex 32`.
2. Put the **same** value in SSM for each env, before applying Terraform:
   `aws ssm put-parameter --type SecureString --name /<app_name>/<env>/AUTH_PORTAL_PROXY_SECRET --value …`
   (staging, and production when `enable_production_host` is on). Do this
   first: while any listed parameter is missing, a redeploy skips the whole SSM
   refresh and the host keeps its old config.
3. Add `AUTH_PORTAL_PROXY_SECRET` with that value to the Amplify auth-portal
   app's environment variables and redeploy it; `amplify.yml` already copies it
   into `.env.production`.
