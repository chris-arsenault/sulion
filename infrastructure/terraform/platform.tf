data "aws_region" "current" {}

module "ctx" {
  source = "git::https://github.com/chris-arsenault/ahara-tf-patterns.git//modules/platform-context"
}

resource "aws_cognito_user_pool_client" "sulion" {
  name                    = local.cognito_client_name
  user_pool_id            = module.ctx.cognito.user_pool_id
  generate_secret         = false
  explicit_auth_flows     = ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH", "ALLOW_USER_SRP_AUTH"]
  access_token_validity   = 5
  id_token_validity       = 5
  refresh_token_validity  = 30
  enable_token_revocation = true
  token_validity_units {
    access_token  = "minutes"
    id_token      = "minutes"
    refresh_token = "days"
  }
}

moved {
  from = module.cognito.aws_cognito_user_pool_client.this
  to   = aws_cognito_user_pool_client.sulion
}

resource "random_password" "secret_broker_registration_token" {
  length           = 48
  special          = false
  override_special = ""
}

resource "random_password" "retrieval_token" {
  length           = 48
  special          = false
  override_special = ""
}

resource "random_password" "code_intel_token" {
  length           = 48
  special          = false
  override_special = ""
}

resource "aws_ssm_parameter" "cognito_client_id" {
  name  = "${local.ssm_prefix}/cognito/clients/${local.cognito_client_name}"
  type  = "String"
  value = aws_cognito_user_pool_client.sulion.id
}

resource "aws_ssm_parameter" "auth_trigger_client" {
  name  = "${local.ssm_prefix}/auth-trigger/clients/${local.auth_trigger_app_name}"
  type  = "String"
  value = aws_cognito_user_pool_client.sulion.id
}

resource "aws_ssm_parameter" "secret_broker_registration_token" {
  name  = "${local.ssm_prefix}/sulion/secret-broker-registration-token"
  type  = "SecureString"
  value = random_password.secret_broker_registration_token.result
}

resource "aws_ssm_parameter" "retrieval_token" {
  name  = "${local.ssm_prefix}/sulion/retrieval-token"
  type  = "SecureString"
  value = random_password.retrieval_token.result
}

resource "aws_ssm_parameter" "code_intel_token" {
  name  = "${local.ssm_prefix}/sulion/code-intel-token"
  type  = "SecureString"
  value = random_password.code_intel_token.result
}
