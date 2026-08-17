FROM docker.io/library/debian:trixie-slim

ARG AWS_SIGNING_HELPER_VERSION=1.8.4
ARG AWS_SIGNING_HELPER_SHA256=b7568acd6e1517a4e1adaee68d52bfd6284a0e5305677166cd83d43a07c815c9

# This service reads its own database URL and registration token at start with
# the identity the trust appliance issues it (ahara-trust ADR-0002). curl and
# openssl are what the enrollment bootstrap speaks with; awscli reads the
# parameters.
RUN apt-get update && apt-get install -y --no-install-recommends \
    awscli \
    bash \
    ca-certificates \
    curl \
    openssl \
    tini \
  && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL \
    "https://rolesanywhere.amazonaws.com/releases/${AWS_SIGNING_HELPER_VERSION}/X86_64/Linux/Amzn2023/aws_signing_helper" \
    -o /usr/local/bin/aws_signing_helper \
    && echo "${AWS_SIGNING_HELPER_SHA256}  /usr/local/bin/aws_signing_helper" | sha256sum -c - \
    && chmod 0755 /usr/local/bin/aws_signing_helper

COPY truenas-roles-anywhere-bootstrap /usr/local/bin/truenas-roles-anywhere-bootstrap
RUN chmod 0755 /usr/local/bin/truenas-roles-anywhere-bootstrap

ARG BROKER_UID=7322
ARG BROKER_GID=7322
RUN groupadd --gid ${BROKER_GID} broker \
  && useradd --uid ${BROKER_UID} --gid ${BROKER_GID} --shell /usr/sbin/nologin --home-dir /var/lib/sulion-broker broker \
  && mkdir -p /var/lib/sulion-broker \
  && chown -R broker:broker /var/lib/sulion-broker

COPY dist/sulion-broker /usr/local/bin/sulion-broker
RUN chmod +x /usr/local/bin/sulion-broker

USER broker
WORKDIR /var/lib/sulion-broker

EXPOSE 8081
ENTRYPOINT ["/usr/bin/tini", "--", \
            "/usr/local/bin/truenas-roles-anywhere-bootstrap", "AWS_RA_BROKER", "--", \
            "/usr/local/bin/sulion-broker"]
