FROM docker.io/library/debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    docker.io \
    tini \
  && rm -rf /var/lib/apt/lists/*

COPY dist/sulion-runner /usr/local/bin/sulion-runner
RUN chmod +x /usr/local/bin/sulion-runner

WORKDIR /home/dev
EXPOSE 8082
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/sulion-runner"]
