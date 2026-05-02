output "cognito_client_id" {
  value = module.cognito.client_id
}

output "cognito_issuer_url" {
  value = "https://cognito-idp.${data.aws_region.current.region}.amazonaws.com/${module.ctx.cognito.user_pool_id}"
}

output "cognito_user_pool_id" {
  value = module.ctx.cognito.user_pool_id
}

output "secret_broker_registration_token_ssm_path" {
  value = aws_ssm_parameter.secret_broker_registration_token.name
}
