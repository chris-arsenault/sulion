# Runtime base. Must have GLIBC >= 2.39 so binaries built on GitHub's
# ubuntu-latest (24.04 → glibc 2.39) load correctly. Bookworm ships
# glibc 2.36 and was rejected at container start with
#   version `GLIBC_2.39' not found.
# Trixie (Debian 13, glibc 2.41) is the current stable and works.
FROM debian:trixie-slim

# Base shell + dev workbench tools. The PTYs that the backend forks
# inherit this image's filesystem, so every tool the user needs in
# their Claude sessions has to be available here.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    wget \
    bash \
    git \
    openssh-client \
    tini \
    vim \
    nano \
    less \
    jq \
    ripgrep \
    fd-find \
    ca-certificates \
    gnupg \
    postgresql-client \
    socat \
  && rm -rf /var/lib/apt/lists/*

# Debian ships fd as fdfind to avoid a name collision; expose the
# expected `fd` binary too.
RUN ln -s /usr/bin/fdfind /usr/local/bin/fd

# Node 20 LTS — Debian's packaged `nodejs` lags behind and breaks a lot
# of modern npm installs (including `@anthropic-ai/claude-code`).
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
  && apt-get update && apt-get install -y --no-install-recommends nodejs \
  && rm -rf /var/lib/apt/lists/*

# pnpm via corepack (bundled with Node 20). The frontend Makefile
# targets invoke `pnpm exec …` directly, so pnpm has to be on PATH
# inside every PTY.
RUN corepack enable \
  && corepack prepare pnpm@9 --activate

# Rust stable toolchain — backend Makefile targets (`cargo check/test/
# clippy/fmt`) assume cargo is on PATH, and PTY sessions doing backend
# work inherit this image. Installed system-wide under /usr/local so
# every user sees it; minimal profile + clippy + rustfmt covers the
# `make lint-rust / test-rust` surface.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential \
      pkg-config \
      libssl-dev \
  && rm -rf /var/lib/apt/lists/* \
  && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
       | sh -s -- -y --default-toolchain stable --profile minimal \
         --component clippy --component rustfmt \
  && chmod -R a+rX /usr/local/rustup /usr/local/cargo

# GitHub CLI from the official apt source — not in Debian's main repos.
RUN mkdir -p -m 755 /etc/apt/keyrings \
  && wget -nv -O- https://cli.github.com/packages/githubcli-archive-keyring.gpg \
       | tee /etc/apt/keyrings/githubcli-archive-keyring.gpg > /dev/null \
  && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
  && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
       > /etc/apt/sources.list.d/github-cli.list \
  && apt-get update \
  && apt-get install -y --no-install-recommends gh \
  && rm -rf /var/lib/apt/lists/*

# Agent CLIs pre-installed system-wide so a fresh PTY can
# `claude login` / `codex login` without any bootstrap. Users who
# want a different version override locally via `npm i -g …` (prefix
# is set to ~/.local in the entrypoint, so user installs shadow these).
RUN npm install -g @anthropic-ai/claude-code @openai/codex

# sulion-managed shell aliases. Goes in /etc/bash.bashrc which
# Debian sources for interactive non-login shells — which is what a
# PTY gets. User can still override in their own ~/.bashrc (runs
# AFTER /etc/bash.bashrc). Keep this list tight; aliases are a
# product surface, not a dumping ground.
RUN { \
      echo ''; \
      echo '# sulion: agent shortcuts'; \
      echo 'alias cl="sulion-agent --type claude --mode real -- --dangerously-skip-permissions"'; \
      echo 'alias co="sulion-agent --type codex --mode real -- --yolo"'; \
    } >> /etc/bash.bashrc

# 7321 is a deliberately unusual UID/GID to avoid colliding with the
# ~1000 range that most consumer apps claim. Must match the ownership
# of the TrueNAS dataset bind-mounted at /home/dev.
ARG DEV_UID=7321
ARG DEV_GID=7321
RUN groupadd --gid ${DEV_GID} dev \
  && useradd --uid ${DEV_UID} --gid ${DEV_GID} --shell /bin/bash --home-dir /home/dev dev \
  && mkdir -p /home/dev \
  && mkdir -p /run/sulion /opt/sulion/hooks /opt/sulion/bin \
  && chown -R dev:dev /home/dev /run/sulion

COPY dist/sulion /usr/local/bin/sulion
COPY bin/sulion-agent /opt/sulion/bin/sulion-agent
COPY hooks/session-start.sh /opt/sulion/hooks/session-start.sh
COPY entrypoint.sh /opt/sulion/entrypoint.sh
RUN chmod +x /usr/local/bin/sulion /opt/sulion/bin/sulion-agent /opt/sulion/hooks/session-start.sh /opt/sulion/entrypoint.sh
USER dev
WORKDIR /home/dev

EXPOSE 8080
ENTRYPOINT ["/usr/bin/tini", "--", "/opt/sulion/entrypoint.sh"]
