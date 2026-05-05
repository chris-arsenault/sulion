FROM docker.io/library/debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    docker.io \
    docker-compose \
    tini \
  && test -x /usr/bin/docker \
  && /usr/bin/docker --version \
  && docker compose version \
  && /usr/bin/docker-compose --version \
  && rm -rf /var/lib/apt/lists/*

ENV SULION_RUNNER_DOCKER_BIN=/usr/bin/docker

COPY dist/sulion-runner /usr/local/bin/sulion-runner
RUN chmod +x /usr/local/bin/sulion-runner

WORKDIR /home/dev
EXPOSE 8082
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/sulion-runner"]
