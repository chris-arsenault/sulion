FROM docker.io/library/debian:trixie-slim

ARG AWS_SIGNING_HELPER_VERSION=1.8.4
ARG AWS_SIGNING_HELPER_SHA256=b7568acd6e1517a4e1adaee68d52bfd6284a0e5305677166cd83d43a07c815c9

# This service reads its own database URL and token at start with the identity
# the trust appliance issues it (ahara-trust ADR-0002). curl and openssl are
# what the enrollment bootstrap speaks with; awscli reads the parameters.
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

ARG RETRIEVAL_UID=7323
ARG RETRIEVAL_GID=7323
RUN groupadd --gid ${RETRIEVAL_GID} retrieval \
  && useradd --uid ${RETRIEVAL_UID} --gid ${RETRIEVAL_GID} --shell /usr/sbin/nologin --home-dir /var/lib/sulion-retrieval retrieval \
  && mkdir -p /var/lib/sulion-retrieval \
  && chown -R retrieval:retrieval /var/lib/sulion-retrieval

COPY dist/sulion-retrieval /usr/local/bin/sulion-retrieval
RUN chmod +x /usr/local/bin/sulion-retrieval

USER retrieval
WORKDIR /var/lib/sulion-retrieval

EXPOSE 8083
ENTRYPOINT ["/usr/bin/tini", "--", \
            "/usr/local/bin/truenas-roles-anywhere-bootstrap", "AWS_RA_RETRIEVAL", "--", \
            "/usr/local/bin/sulion-retrieval"]
