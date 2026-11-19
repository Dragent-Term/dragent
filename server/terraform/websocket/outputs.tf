output "public_ip" {
  value       = module.staging_ec2.public_ip
  description = "Elastic IP — point beta.api.dragent.dev's A record here."
}

output "instance_id" {
  value       = module.staging_ec2.instance_id
  description = "For SSM: aws ssm start-session --target <this>."
}

output "domain" {
  value = module.staging_ec2.domain
}

output "cloudwatch_log_group_name" {
  value       = "/dragent/${var.app_name}/staging/websocket"
  description = "CloudWatch Logs group for bootstrap and Docker container logs."
}

output "production_public_ip" {
  value       = try(module.production_ec2[0].public_ip, null)
  description = "Optional production host Elastic IP; null until enable_production_host is true."
}

output "production_instance_id" {
  value       = try(module.production_ec2[0].instance_id, null)
  description = "Optional production host instance ID for SSM access."
}
