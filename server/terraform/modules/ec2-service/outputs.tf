output "public_ip" {
  value       = aws_eip.server.public_ip
  description = "Elastic IP to point the domain's A record at (DNS cutover)."
}

output "instance_id" {
  value       = aws_instance.server.id
  description = "For SSM Session Manager (`aws ssm start-session --target <id>`)."
}

output "domain" {
  value       = var.domain
  description = "The hostname Caddy serves."
}

output "container_port" {
  value       = var.container_port
  description = "Base localhost port of the blue/green pair (this and +1)."
}

output "deploy_script" {
  value       = local.deploy_script
  description = "Rendered blue/green deploy script, byte-identical to what user_data bootstraps. Consumed by the websocket SSM associations to converge live hosts."
}

output "caddyfile_template" {
  value       = local.caddyfile_template
  description = "Rendered Caddyfile template (with the __UPSTREAM_PORT__ token), byte-identical to what user_data bootstraps. Consumed by the websocket SSM associations."
}
