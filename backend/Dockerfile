FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    bash \
    git \
    openssh-client \
    nodejs \
    npm \
    tini \
  && rm -rf /var/lib/apt/lists/*

ARG DEV_UID=1000
ARG DEV_GID=1000
RUN groupadd --gid ${DEV_GID} dev \
  && useradd --uid ${DEV_UID} --gid ${DEV_GID} --shell /bin/bash --home-dir /home/dev dev \
  && mkdir -p /run/shuttlecraft \
  && chown -R dev:dev /run/shuttlecraft

COPY dist/shuttlecraft /usr/local/bin/shuttlecraft
COPY hooks/session-start.sh /opt/shuttlecraft/hooks/session-start.sh
RUN chmod +x /opt/shuttlecraft/hooks/session-start.sh

USER dev
WORKDIR /home/dev

EXPOSE 8080
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/local/bin/shuttlecraft"]
