locals {
  upload_bucket_name = "${local.prefix}-uploads-${data.aws_caller_identity.workload.account_id}"
}

resource "aws_s3_bucket" "uploads" {
  bucket = local.upload_bucket_name
}

resource "aws_s3_bucket_public_access_block" "uploads" {
  bucket                  = aws_s3_bucket.uploads.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "uploads" {
  bucket = aws_s3_bucket.uploads.id
  rule { object_ownership = "BucketOwnerEnforced" }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "uploads" {
  bucket = aws_s3_bucket.uploads.id
  rule {
    apply_server_side_encryption_by_default { sse_algorithm = "AES256" }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "uploads" {
  bucket = aws_s3_bucket.uploads.id
  rule {
    id     = "expire-staged-files"
    status = "Enabled"
    filter { prefix = "uploads/" }
    expiration { days = 2 }
  }
}

resource "aws_s3_bucket_cors_configuration" "uploads" {
  bucket = aws_s3_bucket.uploads.id
  cors_rule {
    allowed_origins = ["https://${local.public_hostname}"]
    allowed_methods = ["PUT"]
    allowed_headers = ["content-type", "if-none-match", "x-amz-checksum-sha256", "x-amz-sdk-checksum-algorithm", "x-amz-meta-upload-binding"]
    expose_headers  = ["ETag", "x-amz-request-id"]
    max_age_seconds = 300
  }
}

data "aws_iam_policy_document" "uploads_bucket" {
  statement {
    sid       = "RequireTLS"
    effect    = "Deny"
    actions   = ["s3:*"]
    resources = [aws_s3_bucket.uploads.arn, "${aws_s3_bucket.uploads.arn}/*"]
    principals {
      type        = "*"
      identifiers = ["*"]
    }
    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
  statement {
    sid       = "RequireConditionalCreation"
    effect    = "Deny"
    actions   = ["s3:PutObject"]
    resources = ["${aws_s3_bucket.uploads.arn}/uploads/*"]
    principals {
      type        = "*"
      identifiers = ["*"]
    }
    condition {
      test     = "Null"
      variable = "s3:if-none-match"
      values   = ["true"]
    }
  }
}

resource "aws_s3_bucket_policy" "uploads" {
  bucket = aws_s3_bucket.uploads.id
  policy = data.aws_iam_policy_document.uploads_bucket.json
}

data "aws_iam_policy_document" "backend_storage" {
  source_policy_documents = [data.aws_iam_policy_document.archive_backend.json]
  statement {
    sid       = "UploadObjects"
    actions   = ["s3:PutObject", "s3:GetObject"]
    resources = ["${aws_s3_bucket.uploads.arn}/uploads/*"]
  }
  statement {
    sid       = "UploadNotFound"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.uploads.arn]
  }
}

resource "aws_ssm_parameter" "upload_bucket" {
  name  = "${local.ssm_prefix}/sulion/upload-bucket"
  type  = "String"
  value = aws_s3_bucket.uploads.id
}
