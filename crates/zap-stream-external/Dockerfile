FROM voidic/rust-ffmpeg:latest AS build
WORKDIR /app/src
ENV LD_LIBRARY_PATH=$FFMPEG_DIR/lib
ARG CARGO_FEATURES=""
RUN apt update && \
    apt install -y protobuf-compiler
COPY . .
RUN cargo install --path ./crates/zap-stream-external --root /app/build ${CARGO_FEATURES}

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
COPY crates/zap-stream-external/config.yaml /app/config.yaml
ENTRYPOINT ["/app/bin/zap-stream-external"]
