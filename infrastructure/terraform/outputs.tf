output "cognito_client_id" {
  value = module.cognito.client_id
}

output "cognito_issuer_url" {
  value = "https://cognito-idp.${data.aws_region.current.region}.amazonaws.com/${module.ctx.cognito.user_pool_id}"
}

output "cognito_user_pool_id" {
  value = module.ctx.cognito.user_pool_id
}

output "public_url" {
  value = module.edge.url
}

output "secret_broker_registration_token_ssm_path" {
  value = aws_ssm_parameter.secret_broker_registration_token.name
}

output "retrieval_token_ssm_path" {
  value = aws_ssm_parameter.retrieval_token.name
}

output "code_intel_token_ssm_path" {
  value = aws_ssm_parameter.code_intel_token.name
}

output "enclave_admin_ssh_key_secret_name" {
  value = aws_secretsmanager_secret.enclave_admin_ssh_key.name
}
