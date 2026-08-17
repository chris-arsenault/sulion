FROM docker.io/library/debian:trixie-slim

ARG AWS_SIGNING_HELPER_VERSION=1.8.4
ARG AWS_SIGNING_HELPER_SHA256=b7568acd6e1517a4e1adaee68d52bfd6284a0e5305677166cd83d43a07c815c9

RUN apt-get update && apt-get install -y --no-install-recommends \
    awscli \
    bash \
    build-essential \
    ca-certificates \
    curl \
    git \
    gnupg \
    gzip \
    libssl-dev \
    openssl \
    pkg-config \
    tini \
    xz-utils \
  && rm -rf /var/lib/apt/lists/*

# This service reads its own database URL and token at start with the identity
# the trust appliance issues it (ahara-trust ADR-0002).
RUN curl -fsSL \
    "https://rolesanywhere.amazonaws.com/releases/${AWS_SIGNING_HELPER_VERSION}/X86_64/Linux/Amzn2023/aws_signing_helper" \
    -o /usr/local/bin/aws_signing_helper \
    && echo "${AWS_SIGNING_HELPER_SHA256}  /usr/local/bin/aws_signing_helper" | sha256sum -c - \
    && chmod 0755 /usr/local/bin/aws_signing_helper

COPY truenas-roles-anywhere-bootstrap /usr/local/bin/truenas-roles-anywhere-bootstrap
RUN chmod 0755 /usr/local/bin/truenas-roles-anywhere-bootstrap

# Language servers for semantic code intelligence (def/refs). Without these the
# service reports semantic.available=false and falls back to the syntactic
# index. Both must be present at runtime, so they go in the final image.

# Node.js 24 (NodeSource) — runtime for typescript-language-server.
RUN curl -fsSL https://deb.nodesource.com/setup_24.x | bash - \
  && apt-get install -y --no-install-recommends nodejs \
  && rm -rf /var/lib/apt/lists/*

# typescript-language-server (spawned as `typescript-language-server --stdio`)
# plus tsserver from the `typescript` package. Installs to /usr/bin via npm -g.
RUN npm install -g typescript-language-server@^4 typescript@^5 \
  && npm cache clean --force

# Rust semantic navigation needs the real toolchain, not just the
# rust-analyzer binary. rust-analyzer shells out to cargo/rustc for workspace
# metadata, build scripts, proc macros, and standard-library sources.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
  && rustup component add rust-src rust-analyzer \
  && chmod -R a+rwX /usr/local/rustup /usr/local/cargo \
  && find /usr/local/cargo/bin -maxdepth 1 -type f -exec ln -sf {} /usr/local/bin/ \;

# Run as the dev user/group (7321) that owns the repos/workspaces datasets, so
# the indexer can read them. The roots are NFSv4-ACL ZFS datasets where only
# owner@/group@ get list access; a distinct uid lands in everyone@ (traverse but
# not list) and the indexer fails with `read dir /home/dev/repos`. Sharing the
# owner uid is the reliable grant; the repos mount is :ro so write access is
# still denied.
ARG CODE_INTEL_UID=7321
ARG CODE_INTEL_GID=7321
RUN groupadd --gid ${CODE_INTEL_GID} codeintel \
  && useradd --uid ${CODE_INTEL_UID} --gid ${CODE_INTEL_GID} --shell /usr/sbin/nologin --home-dir /var/lib/sulion-code-intel codeintel \
  && mkdir -p /var/lib/sulion-code-intel \
  && chown -R codeintel:codeintel /var/lib/sulion-code-intel

COPY dist/sulion-code-intel /usr/local/bin/sulion-code-intel
RUN chmod +x /usr/local/bin/sulion-code-intel

USER codeintel
WORKDIR /var/lib/sulion-code-intel

EXPOSE 8084
ENTRYPOINT ["/usr/bin/tini", "--", \
            "/usr/local/bin/truenas-roles-anywhere-bootstrap", "AWS_RA_CODE_INTEL", "--", \
            "/usr/local/bin/sulion-code-intel"]
