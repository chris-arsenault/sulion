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

  # This revision makes a project's /ahara/<project> and
  # /ahara/truenas-db/<project> parameter trees the workload's read boundary.
  # Pin that security contract rather than resolving a floating ref at apply.
  source = "git::https://github.com/chris-arsenault/ahara-infra.git//infrastructure/terraform/modules/machine-role?ref=d02a421bc755444dcfb21570e805360496a1ba13"

  prefix = local.prefix
  name   = each.key

  permissions_boundary_arn = (
    "arn:aws:iam::${data.aws_caller_identity.workload.account_id}:policy/pb-${local.prefix}-truenas-workload"
  )
}
