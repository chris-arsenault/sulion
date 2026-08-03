resource "aws_secretsmanager_secret" "enclave_admin_ssh_key" {
  name        = "${local.prefix}-enclave-admin-ssh-key"
  description = "Operator-managed recovery backup of the sulion-enclave SSH administration private key"

  recovery_window_in_days = 30

  lifecycle {
    prevent_destroy = true
  }
}

# The private key is populated and rotated manually from the administration
# workstation. Do not add an aws_secretsmanager_secret_version resource: that
# would place the key material in Terraform configuration and state.
