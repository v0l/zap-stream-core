FROM voidic/rust-ffmpeg:latest AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app/src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
ENV LD_LIBRARY_PATH=$FFMPEG_DIR/lib
# Share the target dir between `cargo chef cook` and `cargo install` so the
# cooked dependency artifacts are reused for the final build.
ENV CARGO_TARGET_DIR=/app/src/target
ARG CARGO_FEATURES=""
RUN apt update && \
    apt install -y protobuf-compiler
COPY --from=planner /app/src/recipe.json recipe.json
# Build dependencies only. This layer is keyed on recipe.json (Cargo.toml +
# Cargo.lock), so CI layer caching reuses it for any commit that only touches
# source code. NOTE: no cache mount over $CARGO_TARGET_DIR here — that would
# shadow the cooked artifacts and defeat layer caching on CI.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=shared \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo chef cook --release -p zap-stream --recipe-path recipe.json ${CARGO_FEATURES}
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=shared \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo install --path ./crates/zap-stream --root /app/build ${CARGO_FEATURES}

FROM debian:trixie-slim
WORKDIR /app
## Install runtime libs
RUN apt update && \
    apt install -y \
    ca-certificates \
    libssl3 \
    libx264-164 \
    libx265-215 \
    libvpx9 \
    libopus0 \
    libwebp7 \
    libwebpmux3 \
    libdav1d7 \
    va-driver-all \
    libva-drm2 \
    libva-x11-2 \
    libva-wayland2 \
    libva-glx2 && \
    if [ "$(dpkg --print-architecture)" == "amd64" ]; then \
        apt install -y libvpl-dev; \
    fi \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/build .
COPY --from=build /app/src/ffmpeg/lib/ /lib
RUN /app/bin/zap-stream --version
ENTRYPOINT ["/app/bin/zap-stream"]
