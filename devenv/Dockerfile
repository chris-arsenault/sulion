# The devenv image: the PTY toolset and nothing per-commit.
#
# This image is content-addressed in CI (platform.yml
# content_addressed_images): its ID changes exactly when this directory
# changes, and the node rolls devenv containers only when the ID does — so a
# backend release leaves running shells alone. That property forbids baking
# per-commit Rust binaries here. The node delivers `sulion-devenv` (the PTY
# server this container runs) and `sulion` (the CLI the wrappers exec)
# through the shared run volume instead; /usr/local/bin/sulion is a symlink
# into that volume, dangling until it is mounted.
#
# The toolset layers are duplicated from backend/Dockerfile (the backend
# image stays a workbench for the combined role); `make validate-deploy`
# keeps the shared PTY assets in bin/, hooks/, docs/, and entrypoint.sh
# byte-identical with backend/'s copies.

# Runtime base. Must be glibc-based so binaries built on GitHub's
# ubuntu-latest run without musl rebuilds. Rocky 10 carries glibc
# 2.39, matching current Ubuntu-built artifacts.
FROM docker.io/rockylinux/rockylinux:10

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG DUCKDB_VERSION=1.5.2
ARG CARGO_XWIN_VERSION=0.22.0

# Base shell + dev workbench tools. Every tool the user needs in their
# Claude sessions has to be available here — PTY shells inherit this
# image's filesystem.
RUN dnf install -y --setopt=install_weak_deps=False dnf-plugins-core epel-release \
  && dnf config-manager --set-enabled crb \
  && dnf install -y --setopt=install_weak_deps=False \
    ca-certificates \
    curl \
    wget \
    bash \
    sudo \
    git \
    git-lfs \
    awscli2 \
    openssh-clients \
    dumb-init \
    vim-enhanced \
    nano \
    less \
    jq \
    ripgrep \
    fd-find \
    ffmpeg-free \
    python3 \
    python3-pip \
    python3-devel \
    gnupg2 \
    postgresql \
    procps-ng \
    findutils \
    which \
    tar \
    gzip \
    xz \
    unzip \
    libgomp \
    socat \
    iproute \
    wireguard-tools \
  && dnf clean all \
  && rm -rf /var/cache/dnf \
  && ln -sf /usr/bin/python3.12 /usr/local/bin/python3 \
  && ln -sf /usr/bin/python3.12 /usr/local/bin/python

# Rocky maintains the CA bundle under /etc/pki, while Nix and other
# cross-distribution tools also probe Debian's conventional bundle path.
RUN install -d -m 0755 /etc/ssl/certs \
  && ln -sfn /etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem \
    /etc/ssl/certs/ca-certificates.crt \
  && test -s /etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem \
  && test -s /etc/ssl/certs/ca-certificates.crt

# Brokered mode uses the constrained runner service. Direct mode uses the
# dedicated host's Docker socket. Install only the CLI and Compose plugin.
RUN dnf config-manager --add-repo https://download.docker.com/linux/centos/docker-ce.repo \
  && dnf install -y --setopt=install_weak_deps=False \
    docker-ce-cli \
    docker-buildx-plugin \
    docker-compose-plugin \
  && dnf clean all \
  && rm -rf /var/cache/dnf

# Python helpers used from PTYs. Keep these system-installed so agents
# can import them immediately from the default shell without creating a
# virtualenv first.
RUN /usr/local/bin/python3 -m pip install --no-cache-dir \
  --upgrade pip setuptools wheel \
  && /usr/local/bin/python3 -m pip install --no-cache-dir \
  click \
  "duckdb==${DUCKDB_VERSION}" \
  Pillow \
  "rembg[cpu]"

# uv is the preferred Python project/environment tool in PTYs. Install
# the standalone binary globally rather than relying on pyenv or a
# repo-local bootstrap.
RUN curl -LsSf https://astral.sh/uv/install.sh \
    | env UV_INSTALL_DIR=/usr/local/bin INSTALLER_NO_MODIFY_PATH=1 sh \
  && chmod 0755 /usr/local/bin/uv /usr/local/bin/uvx

# Some distros expose fd as fdfind to avoid a name collision; expose
# the expected `fd` binary too when needed.
RUN if command -v fdfind >/dev/null 2>&1 && ! command -v fd >/dev/null 2>&1; then \
      ln -s /usr/bin/fdfind /usr/local/bin/fd; \
    fi

# Node 24 LTS from NodeSource; distro Node channels lag behind modern
# npm tooling used by the agent CLIs, and the Ableton Extensions SDK
# pins Node 24.16.0.
RUN curl -fsSL https://rpm.nodesource.com/setup_24.x | bash - \
  && dnf install -y --setopt=install_weak_deps=False nodejs \
  && dnf clean all \
  && rm -rf /var/cache/dnf

# pnpm via corepack (bundled with Node 24). The frontend Makefile
# targets invoke `pnpm exec …` directly, so pnpm has to be on PATH
# inside every PTY.
RUN corepack enable \
  && corepack prepare pnpm@9 --activate

# Rust stable toolchain — backend Makefile targets (`cargo check/test/
# clippy/fmt`) assume cargo is on PATH, and PTY sessions doing backend
# work inherit this image. Installed system-wide under /usr/local so
# every user sees it; minimal profile + clippy + rustfmt covers the
# `make lint-rust / test-rust` surface. Pin shells to the explicit
# host-triple toolchain name too: some environments end up with a
# broken named `1.xx.x-<triple>` toolchain while the healthy
# `stable-<triple>` install still works. Exporting RUSTUP_TOOLCHAIN
# keeps every default shell, make invocation, and agent subprocess on
# the known-good stable toolchain without needing per-command overrides.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu \
    PATH=/usr/local/cargo/bin:$PATH
RUN dnf groupinstall -y "Development Tools" \
  && dnf install -y --setopt=install_weak_deps=False \
      openssl-devel \
      pkgconf-pkg-config \
  && dnf clean all \
  && rm -rf /var/cache/dnf \
  && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
       | sh -s -- -y --default-toolchain stable --profile minimal \
  && rustup toolchain install stable-x86_64-unknown-linux-gnu --profile minimal \
  && rustup component add --toolchain stable-x86_64-unknown-linux-gnu clippy rustfmt rust-analyzer \
  && rustup target add --toolchain stable-x86_64-unknown-linux-gnu \
       x86_64-pc-windows-msvc \
       aarch64-pc-windows-msvc \
  && rustup default stable-x86_64-unknown-linux-gnu \
  && cargo install --locked cargo-xwin --version "${CARGO_XWIN_VERSION}" \
  # a+rwX so non-root PTY users can populate the registry / target
  # caches under /usr/local/cargo. The installer writes everything
  # root-owned; matching the official rust image's "shared toolchain"
  # convention. Without this, `make ci` inside a dev PTY fails on
  # the first crate fetch with "Permission denied".
  && chmod -R a+rwX /usr/local/rustup /usr/local/cargo \
  # Login shells source /etc/profile, which resets PATH and drops
  # /usr/local/cargo/bin. Symlink the rustup proxies into
  # /usr/local/bin so cargo/rustc/clippy/rustfmt stay reachable from
  # the default shell regardless of how PATH is initialised.
  && find /usr/local/cargo/bin -maxdepth 1 -type f -exec ln -sf {} /usr/local/bin/ \;

# .NET SDKs for C# builds in PTYs. Some repos in the shared workspace
# still target .NET 8 while others pin .NET 10 via global.json and
# test projects. Use dotnet-install.sh so both SDKs are present
# side-by-side independent of distro package channels. A /usr/bin/dotnet
# symlink keeps it on PATH for every user. Opt out of Microsoft's
# telemetry by default for a dev image.
ENV DOTNET_ROOT=/usr/share/dotnet \
    DOTNET_CLI_TELEMETRY_OPTOUT=1 \
    DOTNET_NOLOGO=1
RUN dnf install -y --setopt=install_weak_deps=False \
      libicu-devel \
  && dnf clean all \
  && rm -rf /var/cache/dnf \
  && curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
  && chmod +x /tmp/dotnet-install.sh \
  && /tmp/dotnet-install.sh --channel 8.0 --install-dir "${DOTNET_ROOT}" \
  && /tmp/dotnet-install.sh --version 10.0.100 --install-dir "${DOTNET_ROOT}" \
  && rm -f /tmp/dotnet-install.sh \
  && ln -s "${DOTNET_ROOT}/dotnet" /usr/local/bin/dotnet

# Terraform CLI for repo-local infrastructure work inside PTYs. The
# platform integration docs require terraform in the project tree, and
# login shells already include /usr/local/bin on PATH, so install the
# standalone binary there rather than depending on distro packaging.
ARG TERRAFORM_VERSION=1.15.2
RUN arch="$(uname -m)" \
  && case "${arch}" in \
       x86_64) tf_arch=amd64 ;; \
       aarch64) tf_arch=arm64 ;; \
       *) echo "unsupported terraform arch: ${arch}" >&2; exit 1 ;; \
     esac \
  && curl -fsSL "https://releases.hashicorp.com/terraform/${TERRAFORM_VERSION}/terraform_${TERRAFORM_VERSION}_linux_${tf_arch}.zip" \
       -o /tmp/terraform.zip \
  && unzip -q /tmp/terraform.zip -d /usr/local/bin \
  && rm -f /tmp/terraform.zip \
  && chmod 0755 /usr/local/bin/terraform

# DuckDB for local data inspection from PTYs. Install both the Python
# binding above and the standalone CLI here so `python3 -c 'import
# duckdb'` and `duckdb <db>` work without a project virtualenv.
RUN arch="$(uname -m)" \
  && case "${arch}" in \
       x86_64) duckdb_arch=amd64 ;; \
       aarch64) duckdb_arch=arm64 ;; \
       *) echo "unsupported duckdb arch: ${arch}" >&2; exit 1 ;; \
     esac \
  && mkdir -p /tmp/duckdb \
  && curl -fsSL "https://github.com/duckdb/duckdb/releases/download/v${DUCKDB_VERSION}/duckdb_cli-linux-${duckdb_arch}.zip" \
       -o /tmp/duckdb.zip \
  && unzip -q /tmp/duckdb.zip -d /tmp/duckdb \
  && install -m 0755 /tmp/duckdb/duckdb /usr/local/bin/duckdb \
  && rm -rf /tmp/duckdb /tmp/duckdb.zip

# GitHub CLI from the official RPM repo.
RUN dnf config-manager --add-repo https://cli.github.com/packages/rpm/gh-cli.repo \
  && dnf install -y --setopt=install_weak_deps=False gh \
  && dnf clean all \
  && rm -rf /var/cache/dnf

# Ruby via RVM for PTYs working on Ruby projects. Installed multi-user under
# /usr/local/rvm so the toolchain is shared; the `sulion` user joins the `rvm`
# group below so it can install gems/rubies and `rvm use` other versions
# (RVM's shell integration loads from /etc/profile.d/rvm.sh in login shells).
# The default ruby/gem/bundle wrappers are symlinked into /usr/local/bin so
# they stay on PATH even when /etc/profile resets it, matching the cargo and
# dotnet handling above. Build deps for compiling Ruby from source are
# installed explicitly ("Development Tools" + openssl-devel already came in
# with the Rust block).
ARG RUBY_VERSION=3.4.4
RUN dnf install -y --setopt=install_weak_deps=False \
      readline-devel \
      zlib-devel \
      libyaml-devel \
      libffi-devel \
      gdbm-devel \
      ncurses-devel \
      bzip2 \
  && dnf clean all \
  && rm -rf /var/cache/dnf \
  && curl -sSL https://rvm.io/mpapis.asc | gpg2 --import - \
  && curl -sSL https://rvm.io/pkuczynski.asc | gpg2 --import - \
  && curl -sSL https://get.rvm.io | bash -s stable \
  && source /etc/profile.d/rvm.sh \
  && rvm install "${RUBY_VERSION}" \
  && rvm --default use "${RUBY_VERSION}" \
  && rvm cleanup all \
  && for w in ruby gem bundle bundler irb rake; do \
       ln -sf "/usr/local/rvm/wrappers/default/$w" "/usr/local/bin/$w"; \
     done \
  && chmod -R g+rwX /usr/local/rvm

# Agent CLIs pre-installed system-wide so a fresh PTY can
# `claude login` / `codex login` without any bootstrap. Users who
# want a different version override locally via `npm i -g …` (prefix
# is set to ~/.local in the entrypoint, so user installs shadow these).
RUN npm install -g @anthropic-ai/claude-code @openai/codex typescript typescript-language-server

# 7321 is a deliberately unusual UID/GID to avoid colliding with the
# ~1000 range that most consumer apps claim. Must match the ownership of the
# bind-mounted home dataset. One identity everywhere: the account is named
# `sulion` in the image, on the dedicated host, and in every mounted path —
# no per-role aliasing, because dual naming has produced real bugs.
ARG DEV_UID=7321
ARG DEV_GID=7321
RUN groupadd --gid ${DEV_GID} sulion \
  && useradd --uid ${DEV_UID} --gid ${DEV_GID} --shell /bin/bash --home-dir /home/sulion sulion \
  && usermod -aG rvm sulion \
  && mkdir -p /home/sulion \
  && mkdir -p /run/sulion /opt/sulion/hooks /opt/sulion/bin /opt/sulion/docs \
  && echo 'sulion ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/sulion \
  && chmod 0440 /etc/sudoers.d/sulion \
  && chown -R sulion:sulion /home/sulion /run/sulion

# Nix, so the flake and NixOS modules under nix/ can be checked from a PTY
# directly. `make validate-nix` already covers them, but it does so by building
# nix/Dockerfile.check, which pulls a nixos/nix image and re-copies the tree on
# every run — too slow to reach for while editing. A local `nix-instantiate
# --parse` answers in milliseconds, so the fast check exists alongside the
# thorough one rather than replacing it: validate-nix still runs `flake check`
# and stays the gate.
#
# Single-user install: the daemon needs systemd, which these containers do not
# run. `build-users-group` must be emptied in /etc/nix/nix.conf *before* the
# installer runs, or it fails looking for the `nixbld` group it did not create.
#
# The store is chowned to `sulion` rather than made world-writable like
# /usr/local/cargo, because nix chmods /nix/var/nix/profiles/per-user on every
# invocation and chmod requires ownership — a+rwX is not enough and the command
# fails for a non-root user. That is also why this block sits after the account
# exists. Root still reaches the store by being root.
#
# Symlinked into /usr/local/bin for the same reason cargo is: login shells
# source /etc/profile, which resets PATH and would drop the profile's bin. Each
# name is linked separately because the nix binary dispatches on argv[0].
RUN mkdir -m 0755 /nix \
  && mkdir -p /etc/nix \
  && printf 'experimental-features = nix-command flakes\nbuild-users-group =\n' \
       > /etc/nix/nix.conf \
  && curl --proto '=https' --tlsv1.2 -sSf -L https://nixos.org/nix/install \
       | sh -s -- --no-daemon --no-channel-add \
  && for b in /root/.nix-profile/bin/*; do \
       ln -sf "$(readlink -f "$b")" "/usr/local/bin/$(basename "$b")"; \
     done \
  && chown -R sulion:sulion /nix

# The per-commit binaries live on the shared run volume, delivered by the
# node: `sulion-devenv` is exec'd by the launcher at a versioned path, and
# the `sulion` CLI the wrappers exec resolves through this stable symlink
# (dangling until the volume is mounted).
RUN ln -s /run/sulion/bin/sulion /usr/local/bin/sulion

COPY bin/sulion-agent /opt/sulion/bin/sulion-agent
COPY bin/cl /opt/sulion/bin/cl
COPY bin/co /opt/sulion/bin/co
COPY bin/docker /opt/sulion/bin/docker
COPY bin/docker-compose /opt/sulion/bin/docker-compose
COPY bin/aws /opt/sulion/bin/aws
COPY bin/with-cred /opt/sulion/bin/with-cred
COPY bin/sulion-retrieve /opt/sulion/bin/sulion-retrieve
COPY bin/sulion-code /opt/sulion/bin/sulion-code
COPY docs/toolset.md /opt/sulion/docs/toolset.md
COPY hooks/session-start.sh /opt/sulion/hooks/session-start.sh
COPY hooks/activity-hook.sh /opt/sulion/hooks/activity-hook.sh
COPY entrypoint.sh /opt/sulion/entrypoint.sh
RUN chmod +x /opt/sulion/bin/sulion-agent /opt/sulion/bin/cl /opt/sulion/bin/co /opt/sulion/bin/docker /opt/sulion/bin/docker-compose /opt/sulion/bin/aws /opt/sulion/bin/with-cred /opt/sulion/bin/sulion-retrieve /opt/sulion/bin/sulion-code /opt/sulion/hooks/session-start.sh /opt/sulion/hooks/activity-hook.sh /opt/sulion/entrypoint.sh
USER sulion
WORKDIR /home/sulion

ENTRYPOINT ["/usr/bin/dumb-init", "--", "/opt/sulion/entrypoint.sh"]
