FROM docker.io/library/debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tini \
  && rm -rf /var/lib/apt/lists/*

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
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/sulion-broker"]
