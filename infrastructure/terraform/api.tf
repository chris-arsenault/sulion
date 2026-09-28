data "aws_lb_target_group" "reverse_proxy" {
  name = "ahara-proxy-tg"
}

# Keep application claims and routing local to Sulion; the pinned shared module
# owns DNS and certificates.
resource "aws_lb_listener_rule" "browser_auth" {
  listener_arn = module.ctx.alb.listener_arn
  priority     = 177
  condition {
    host_header { values = [local.public_hostname] }
  }
  condition {
    path_pattern { values = ["/api/*", "/broker/*"] }
  }
  action {
    type = "jwt-validation"
    jwt_validation {
      issuer        = module.ctx.cognito.issuer
      jwks_endpoint = module.ctx.cognito.jwks
      additional_claim {
        name   = "client_id"
        format = "single-string"
        values = [aws_cognito_user_pool_client.sulion.id]
      }
      additional_claim {
        name   = "token_use"
        format = "single-string"
        values = ["access"]
      }
    }
  }
  action {
    type             = "forward"
    target_group_arn = data.aws_lb_target_group.reverse_proxy.arn
  }
}

moved {
  from = module.edge.aws_lb_listener_rule.this["0"]
  to   = aws_lb_listener_rule.private_paths["173"]
}
moved {
  from = module.edge.aws_lb_listener_rule.this["1"]
  to   = aws_lb_listener_rule.private_paths["174"]
}
moved {
  from = module.edge.aws_lb_listener_rule.this["2"]
  to   = aws_lb_listener_rule.public_paths["175"]
}
moved {
  from = module.edge.aws_lb_listener_rule.this["3"]
  to   = aws_lb_listener_rule.public_paths["176"]
}
moved {
  from = module.edge.aws_lb_listener_rule.this["4"]
  to   = aws_lb_listener_rule.browser_auth
}
moved {
  from = module.edge.aws_lb_listener_rule.this["5"]
  to   = aws_lb_listener_rule.public_paths["178"]
}

# Denial precedes every forward, including the unauthenticated static catch-all.
resource "aws_lb_listener_rule" "private_paths" {
  for_each = {
    "172" = ["/broker/v1/auth/*", "/retrieval", "/retrieval/*"]
    "173" = ["/api/devices/*", "/api/repos/*/ingest", "/api/repos/*/raw"]
    "174" = ["/broker/v1/use", "/broker/v1/pty-credentials", "/broker/v1/pty-credentials/*"]
  }
  listener_arn = module.ctx.alb.listener_arn
  priority     = tonumber(each.key)
  condition {
    host_header { values = [local.public_hostname] }
  }
  condition {
    path_pattern { values = each.value }
  }
  action {
    type = "fixed-response"
    fixed_response {
      content_type = "text/plain"
      status_code  = "404"
      message_body = "Not found"
    }
  }
}

module "edge" {
  source = "git::https://github.com/chris-arsenault/ahara-tf-patterns.git//modules/alb-api-truenas?ref=7891b157df1cb83167eef2c9dfd8a25a6013cb75"

  hostname         = local.public_hostname
  alb              = module.ctx.alb
  cognito          = module.ctx.cognito
  target_group_arn = data.aws_lb_target_group.reverse_proxy.arn

  routes = []
}

resource "aws_lb_listener_rule" "public_paths" {
  for_each = {
    "175" = ["/ws/*"]
    "176" = ["/health"]
    "178" = ["/*"]
  }
  listener_arn = module.ctx.alb.listener_arn
  priority     = tonumber(each.key)
  condition {
    host_header { values = [local.public_hostname] }
  }
  condition {
    path_pattern { values = each.value }
  }
  action {
    type             = "forward"
    target_group_arn = data.aws_lb_target_group.reverse_proxy.arn
  }
}
