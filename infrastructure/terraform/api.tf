data "aws_lb_target_group" "reverse_proxy" {
  name = "ahara-proxy-tg"
}

module "edge" {
  source = "git::https://github.com/chris-arsenault/ahara-tf-patterns.git//modules/alb-api-truenas"

  hostname         = local.public_hostname
  alb              = module.ctx.alb
  cognito          = module.ctx.cognito
  target_group_arn = data.aws_lb_target_group.reverse_proxy.arn

  routes = [
    # Device enrollment starts without a credential; ingest uses Sulion's
    # long-lived device token rather than a Cognito JWT.
    {
      priority = 173
      paths = [
        "/api/devices/pair",
        "/api/devices/pair/token",
        "/api/repos/*/ingest",
        "/api/repos/*/raw",
      ]
      authenticated = false
    },
    # Nodes and PTY wrappers authenticate these routes with their own signed
    # credentials or bearer token rather than a browser Cognito JWT.
    {
      priority = 174
      paths = [
        "/broker/v1/use",
        "/broker/v1/pty-credentials",
        "/broker/v1/pty-credentials/*",
        "/retrieval/*",
      ]
      authenticated = false
    },
    # Browsers cannot attach Authorization headers to a WebSocket handshake.
    # Sulion authenticates this route with a short-lived, one-use ticket.
    {
      priority      = 175
      paths         = ["/ws/*"]
      authenticated = false
    },
    {
      priority      = 176
      paths         = ["/health"]
      authenticated = false
    },
    # Defense in depth: the ALB validates Cognito JWTs before Sulion validates
    # issuer, client, and claims again at the application boundary.
    {
      priority      = 177
      paths         = ["/api/*", "/broker/*"]
      authenticated = true
    },
    # Static assets and the login/pairing shell must load before authentication.
    {
      priority      = 178
      paths         = ["/*"]
      authenticated = false
    },
  ]
}
