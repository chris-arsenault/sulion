variable "plan_mcp" {
  description = "Plans connector grant and exact browser OAuth callback URLs."
  type = object({
    subject       = string
    repos         = list(string)
    callback_urls = list(string)
  })
  default = null

  validation {
    condition = var.plan_mcp == null ? true : (
      trimspace(var.plan_mcp.subject) != "" &&
      length(var.plan_mcp.repos) > 0 &&
      length(var.plan_mcp.callback_urls) > 0
    )
    error_message = "Plan MCP requires a Cognito subject, at least one repository, and at least one OAuth callback."
  }
}

resource "aws_cognito_user_pool_client" "plan_mcp" {
  count = var.plan_mcp == null ? 0 : 1

  name                                 = "sulion-plan-mcp"
  user_pool_id                         = module.ctx.cognito.user_pool_id
  generate_secret                      = true
  allowed_oauth_flows_user_pool_client = true
  allowed_oauth_flows                  = ["code"]
  allowed_oauth_scopes                 = ["openid", "email", "profile"]
  supported_identity_providers         = ["COGNITO"]
  callback_urls                        = var.plan_mcp.callback_urls
  enable_token_revocation              = true
}

resource "aws_ssm_parameter" "plan_mcp_auth_trigger_client" {
  count = var.plan_mcp == null ? 0 : 1

  name  = "${local.ssm_prefix}/auth-trigger/clients/sulion-plan-mcp"
  type  = "String"
  value = aws_cognito_user_pool_client.plan_mcp[0].id
}

resource "aws_ssm_parameter" "plan_mcp_config" {
  name = "${local.ssm_prefix}/sulion/plan-mcp-config"
  type = "String"
  value = var.plan_mcp == null ? "null" : jsonencode({
    resource  = "https://${local.public_hostname}/mcp"
    client_id = aws_cognito_user_pool_client.plan_mcp[0].id
    subject   = var.plan_mcp.subject
    repos     = var.plan_mcp.repos
  })
}
