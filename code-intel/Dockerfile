FROM docker.io/library/debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tini \
  && rm -rf /var/lib/apt/lists/*

ARG CODE_INTEL_UID=7324
ARG CODE_INTEL_GID=7324
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
