# DISABLED alongside main.tf — production runs on Neon, not RDS.
# These outputs reference resources that are commented out; uncomment
# them together with main.tf.

/*
output "endpoint" {
  value       = aws_db_instance.main.address
  description = "Hostname of the instance."
}

output "port" {
  value = aws_db_instance.main.port
}

output "db_name" {
  value = aws_db_instance.main.db_name
}

output "security_group_id" {
  value       = aws_security_group.db.id
  description = "Attach this to anything else that needs Postgres access."
}

output "ssm_database_url_name" {
  value       = aws_ssm_parameter.database_url.name
  description = "SSM parameter holding the full connection URL."
}

# Marked sensitive so it is not echoed by `terraform apply`. To read it:
# Prefer pulling from SSM (`aws ssm get-parameter --with-decryption`) so the
# value is never copied by hand — a mangled connection URL has caused a total
# outage in this project before.
output "database_url" {
  value       = local.database_url
  sensitive   = true
  description = "Full postgres:// URL. Also written to SSM."
}
*/
