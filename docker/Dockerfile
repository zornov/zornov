# The Linux build environment (macOS and Windows build natively on their own
# CI runners: containers only run Linux): Rust, the system libraries Zornov links to,
# and a software Vulkan driver so the GPU tests run without a graphics card.
# CI builds and tests Linux in it, and the dev container (.devcontainer) opens
# the repository in it. The source is mounted, not copied:
#
#   docker build -t zornov-linux docker
#   docker run --rm -it -v "$PWD:/work" zornov-linux cargo test --workspace
#
# The base is pinned by digest; update the tag and digest together.
FROM rust:1.99.0-trixie@sha256:81015672d6939b0265f09adf96d5de125482bb1330106076956b0518e74edc90

# Package versions follow the pinned base image (exact apt versions break
# the build as soon as Debian ships a security fix).
# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      libasound2-dev \
      libfontconfig-dev \
      libxkbcommon-dev \
      libwayland-dev \
      libx11-dev \
      libxcursor-dev \
      libxi-dev \
      libxrandr-dev \
      mesa-vulkan-drivers \
      git \
      gcc-mingw-w64-x86-64 \
 && rm -rf /var/lib/apt/lists/*

# The Windows target, for a quick compile check of the Windows code paths
# (cargo check --target x86_64-pc-windows-gnu); Windows itself builds and
# tests on its own CI runner.
RUN rustup component add clippy rustfmt \
 && rustup target add x86_64-pc-windows-gnu

# Any user can build: CI runs as the runner's user, the dev container as
# `dev`, and cargo writes its caches under CARGO_HOME.
RUN mkdir -p "$CARGO_HOME/registry" "$CARGO_HOME/git" \
 && chmod -R a+rwX "$CARGO_HOME" "$RUSTUP_HOME" \
 && useradd --create-home --uid 1000 --shell /bin/bash dev \
 && install -d -m 1777 /cache /cache/target /cache/home \
 && git config --system --add safe.directory /work \
 && git config --system --add safe.directory '/workspaces/*'

# Linux build output stays apart from a host checkout's own target/ folder.
ENV CARGO_TARGET_DIR=/cache/target
WORKDIR /work
