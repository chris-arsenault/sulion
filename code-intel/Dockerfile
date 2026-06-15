FROM docker.io/library/debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    gnupg \
    gzip \
    tini \
  && rm -rf /var/lib/apt/lists/*

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

# rust-analyzer LSP binary (spawned as `rust-analyzer`), onto /usr/local/bin.
RUN curl -fsSL https://github.com/rust-lang/rust-analyzer/releases/latest/download/rust-analyzer-x86_64-unknown-linux-gnu.gz \
      | gunzip -c > /usr/local/bin/rust-analyzer \
  && chmod +x /usr/local/bin/rust-analyzer

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
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/sulion-code-intel"]
