# Production database (RDS Postgres) — DISABLED

> **Not in use.** Production runs on **Neon**. `main.tf` and `outputs.tf` are
> commented out, so `terraform apply` here creates nothing; `plan` reports
> "No changes". Re-enable by removing the `/*` … `*/` wrappers in both files.
>
> Kept because it is written, validated, and plans clean — the decision was
> economic, not technical. At zero users RDS costs ~$17.60/month for managed
> backups and PITR that Neon's free tier gives at $0. Revisit when Neon's caps
> (100 CU-hours and 0.5 GB per project) start to bite, since exceeding those
> **suspends** the database rather than billing for the overage.


Creates the production Postgres instance, its subnet group, and a security group
that admits only the API host. Writes the connection URL to
`/dragent/production/DATABASE_URL`.

It deliberately does **not** touch anything currently serving. The live server
reads `/dragent/staging/DATABASE_URL`, which this config never writes, so
`apply` is safe to run while the current stack is up. Cutover is manual — below.

Own state, like `../websocket`, because the root config's state was lost and
running it would try to recreate the live shared infra.

---

## Before the first apply

### 1. The current IAM user cannot create RDS

Verified: `rds:DescribeDBEngineVersions` and `rds:DescribeDBInstances` are both
denied for `arn:aws:iam::269235677023:user/Dragent`. `apply` will fail on the
first RDS call until the following are granted:

```
rds:CreateDBInstance          rds:DescribeDBInstances
rds:ModifyDBInstance          rds:DescribeDBEngineVersions
rds:DeleteDBInstance          rds:DescribeOrderableDBInstanceOptions
rds:CreateDBSubnetGroup       rds:DescribeDBSubnetGroups
rds:DeleteDBSubnetGroup       rds:ListTagsForResource
rds:AddTagsToResource         rds:CreateDBSnapshot
```

EC2 (describe instances/subnets/security groups), SSM (`PutParameter`), and KMS
(`Encrypt`/`Decrypt` for the SecureString and storage encryption) are already
in place — this config's data sources were resolved with the current user.

### 2. Confirm the Postgres major version is offered

`engine_version` is `"17"` — major only, so RDS picks the current minor and an
apply cannot fail on a retired one. Once the IAM permissions above exist:

```sh
aws rds describe-db-engine-versions --engine postgres \
  --query 'DBEngineVersions[].EngineVersion' --output text | tr '\t' '\n' | sort -V | tail
```

Match the major to whatever the existing Neon database runs if you plan to
`pg_dump`/`restore` rather than migrate from scratch.

---

## Apply

```sh
terraform -chdir=server/terraform/database init
terraform -chdir=server/terraform/database plan
terraform -chdir=server/terraform/database apply
```

Creation takes roughly 5–10 minutes.

---

## Migrations: read this before flipping `publicly_accessible`

`publicly_accessible` defaults to **false**, so the instance is reachable only
from the API host's security group. That is the right posture — and it breaks
the current deploy workflow.

`.github/workflows/deploy.yml` runs `goose -dir migrations up` from a GitHub
Actions runner using `secrets.DATABASE_URL`. A runner cannot reach a private
instance, so that step will hang and time out.

Three ways out, best first:

1. **Run migrations on the EC2 host.** It already has network access, the
   image, and the connection URL. Drive it over SSM — no inbound access, no
   public endpoint, nothing to open:

   ```sh
   aws ssm send-command --instance-ids i-… --document-name AWS-RunShellScript \
     --parameters 'commands=["docker run --rm --env-file /etc/dragent/server.env <image> goose up"]'
   ```

   Requires a `goose` binary (or a migrate subcommand) in the server image.

2. **Self-hosted runner inside the VPC.** Clean, but a runner to maintain.

3. **`publicly_accessible = true` plus `extra_ingress_cidrs`.** Simplest, and
   the weakest: GitHub's runner IP ranges are large and change, so in practice
   this means a wide allowlist on a database holding billing records. The
   variable refuses `0.0.0.0/0`, but a broad range is barely better. Treat as a
   short-term measure.

---

## Cutover (deliberate, not part of `apply`)

1. Apply this config; confirm `/dragent/production/DATABASE_URL` exists.
2. Run migrations against the new database (above).
3. Seed the live LemonSqueezy variant ids:
   `server/scripts/seed_ls_variants_live.sql` — run **once**, production only.
   It is not a goose migration on purpose: live variant ids must never replay
   into a staging database.
4. Copy the value into the parameter the server actually reads:

   ```sh
   aws ssm get-parameter --name /dragent/production/DATABASE_URL --with-decryption \
     --query Parameter.Value --output text |
   xargs -0 aws ssm put-parameter --name /dragent/staging/DATABASE_URL \
     --type SecureString --overwrite --value
   ```

   Piped rather than pasted, deliberately: a hand-copied connection URL has
   already caused a total outage in this project (see `app/src/api/backend.rs`).

5. Rotate `JWT_SECRET` at the same time — cheap now, and it invalidates every
   token minted during the test-mode era.
6. Restart the container so it re-reads `/etc/dragent/server.env`.
7. Smoke test: sign up → sign in → one agent turn → one real purchase →
   confirm the webhook credits the account.

The old Neon database is left untouched and can be kept as a fallback.

---

## Notes on the defaults

| Setting | Default | Why |
|---|---|---|
| `instance_class` | `db.t4g.micro` | Cheapest current-gen. Scaling up is an in-place modify with a short restart. |
| `multi_az` | `false` | Roughly doubles cost; not worth it before there is traffic. |
| `deletion_protection` | `true` | Customer and billing records. |
| `skip_final_snapshot` | `false` | Destroy still leaves a restorable snapshot. |
| `backup_retention_days` | `7` | Validation rejects `0`. |
| password | 32 chars, **no symbols** | It is interpolated into a `postgres://` URL; `/ @ ? #` would need percent-encoding, and mangled URLs are a known failure mode here. |
