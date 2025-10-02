FROM voidic/rust-ffmpeg:latest AS build
WORKDIR /app/src
ENV LD_LIBRARY_PATH=$FFMPEG_DIR/lib
ARG CARGO_FEATURES=""
RUN apt update && \
    apt install -y protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/src/target \
    cargo fetch
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/src/target \
    cargo install --path ./crates/zap-stream --root /app/build ${CARGO_FEATURES}

FROM rust:trixie
WORKDIR /app
## Install runtime libs
RUN apt update && \
    apt install -y \
    libx264-164 \
    libx265-215 \
    libvpx9 \
    libopus0 \
    libwebp7 \
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
ENTRYPOINT ["/app/bin/zap-stream"]