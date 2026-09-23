# Machine identities for the containers that hold secrets.
#
# Each reads its own database URL and service tokens at start with the
# certificate the trust appliance issued it, rather than receiving values the
# deploy pipeline resolved into Komodo (ahara-trust ADR-0002).
#
# Only TrueNAS workloads appear here. node and ingester run on the dedicated
# host and enroll for nothing: they receive the values they need over the
# authenticated node channel after an operator approves the node (ADR-0002).
# code-intel is listed because the standalone role runs it on TrueNAS; in the
# split topology it runs beside the node and is delivered to in the same way.
# runner and frontend hold no secret and appear here not at all.
#
# Reading this project's parameters is all most of them do with credentials,
# and machine-role derives that from the prefix. The backend alone carries one
# more grant: the transcript archive bucket in archive.tf, which its control
# process writes to and reads back from.

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

  policy_json = each.key == "backend" ? data.aws_iam_policy_document.archive_backend.json : null

  permissions_boundary_arn = (
    "arn:aws:iam::${data.aws_caller_identity.workload.account_id}:policy/pb-${local.prefix}-truenas-workload"
  )
}
