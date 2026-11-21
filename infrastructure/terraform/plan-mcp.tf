module "cognito_plan_mcp" {
  source = "git::https://github.com/chris-arsenault/ahara-tf-patterns.git//modules/cognito-app"
  name   = "sulion-plan-mcp"
  callback_urls = [
    "https://claude.ai/api/mcp/auth_callback",
    "https://chatgpt.com/connector/oauth/Lm5TBtxM2lQP",
  ]
  cognito = module.ctx.cognito
}

moved {
  from = aws_cognito_user_pool_client.plan_mcp[0]
  to   = module.cognito_plan_mcp.aws_cognito_user_pool_client.this
}

resource "aws_ssm_parameter" "plan_mcp_client_id" {
  name  = "${local.ssm_prefix}/cognito/clients/sulion-plan-mcp"
  type  = "String"
  value = module.cognito_plan_mcp.client_id
}

output "plan_mcp_client_id" {
  value = module.cognito_plan_mcp.client_id
}

output "plan_mcp_client_secret" {
  value     = module.cognito_plan_mcp.client_secret
  sensitive = true
}
