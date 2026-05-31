FROM docker.io/library/debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tini \
  && rm -rf /var/lib/apt/lists/*

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
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/sulion-retrieval"]
