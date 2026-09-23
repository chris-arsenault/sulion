# Where archived transcript sessions and the durable database dump go.
#
# The control process exports each idle agent session from `events.payload`
# as one JSON-lines object, dumps the durable tables monthly, and only then
# purges the session's derived rows (docs/plans/transcript-archive-and-purge.md).
# Losing this bucket loses the only copy of purged transcript detail, so it is
# versioned and never expires current objects.
#
# The name matches the `ahara-sulion-*` pattern the shared TrueNAS workload
# permissions boundary already allows S3 access on, so the backend's machine
# role can be granted below without a change in ahara-infra. Encryption is the
# bucket's own SSE-S3: transcripts are code and prompts in a private,
# versioned, TLS-only bucket, and a project KMS key would need the boundary
# widened first.

locals {
  archive_bucket_name = "ahara-${local.prefix}-archive-${data.aws_caller_identity.workload.account_id}"
}

resource "aws_s3_bucket" "archive" {
  bucket = local.archive_bucket_name

  lifecycle {
    prevent_destroy = true
  }
}

# Object history is the only rollback for an export that replaced a good
# object with a bad one, so it lives here rather than in the application.
resource "aws_s3_bucket_versioning" "archive" {
  bucket = aws_s3_bucket.archive.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "archive" {
  bucket = aws_s3_bucket.archive.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
    bucket_key_enabled = true
  }
}

resource "aws_s3_bucket_public_access_block" "archive" {
  bucket                  = aws_s3_bucket.archive.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# Restore reads objects back on demand, so the cold tier must answer in
# milliseconds: Glacier Instant Retrieval, never Deep Archive. Current objects
# are kept forever; superseded versions and abandoned uploads are not.
resource "aws_s3_bucket_lifecycle_configuration" "archive" {
  bucket = aws_s3_bucket.archive.id

  rule {
    id     = "cold-after-a-month"
    status = "Enabled"
    filter {}

    transition {
      days          = 30
      storage_class = "GLACIER_IR"
    }

    noncurrent_version_expiration {
      noncurrent_days = 90
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

data "aws_iam_policy_document" "archive_bucket" {
  statement {
    sid    = "RequireTls"
    effect = "Deny"
    principals {
      type        = "*"
      identifiers = ["*"]
    }
    actions = ["s3:*"]
    resources = [
      aws_s3_bucket.archive.arn,
      "${aws_s3_bucket.archive.arn}/*",
    ]
    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
}

resource "aws_s3_bucket_policy" "archive" {
  bucket = aws_s3_bucket.archive.id
  policy = data.aws_iam_policy_document.archive_bucket.json

  depends_on = [aws_s3_bucket_public_access_block.archive]
}

# What the control process may do with its identity, beyond reading its own
# parameters: write, read back, and list this one bucket. No delete — the
# application never removes an object; lifecycle rules do.
data "aws_iam_policy_document" "archive_backend" {
  statement {
    sid    = "ArchiveObjects"
    effect = "Allow"
    actions = [
      "s3:PutObject",
      "s3:GetObject",
      "s3:AbortMultipartUpload",
      "s3:ListMultipartUploadParts",
    ]
    resources = ["${aws_s3_bucket.archive.arn}/*"]
  }

  statement {
    sid    = "FindTheBucket"
    effect = "Allow"
    actions = [
      "s3:ListBucket",
      "s3:ListBucketMultipartUploads",
      "s3:GetBucketLocation",
    ]
    resources = [aws_s3_bucket.archive.arn]
  }
}

# A bucket name is a public identifier, so the deploy resolves it into the
# stack environment the same way it does the Cognito ids (secret-paths.yml).
resource "aws_ssm_parameter" "archive_bucket" {
  name  = "${local.ssm_prefix}/${local.prefix}/archive-bucket"
  type  = "String"
  value = aws_s3_bucket.archive.id
}

output "archive_bucket" {
  description = "Bucket holding archived transcript sessions and durable database dumps."
  value       = aws_s3_bucket.archive.id
}
