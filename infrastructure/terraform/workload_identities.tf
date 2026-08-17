# Machine identities for the containers that hold secrets.
#
# Each reads its own database URL and service tokens at start with the
# certificate the trust appliance issued it, rather than receiving values the
# deploy pipeline resolved into Komodo (ahara-trust ADR-0002).
#
# backend, node and ingester share the backend image and the backend identity:
# same stack, same host, same image, so separate identities would distinguish
# nothing. runner and frontend hold no secret and appear here not at all.
#
# No policy is passed to any of them: reading this project's parameters is all
# they do with credentials, and machine-role derives that from the prefix.

data "aws_caller_identity" "workload" {}

module "workload_role" {
  for_each = toset([
    "backend",
    "broker",
    "retrieval",
    "code-intel",
  ])

  source = "git::https://github.com/chris-arsenault/ahara-infra.git//infrastructure/terraform/modules/machine-role?ref=main"

  prefix = local.prefix
  name   = each.key

  permissions_boundary_arn = (
    "arn:aws:iam::${data.aws_caller_identity.workload.account_id}:policy/pb-${local.prefix}-truenas-workload"
  )
}
