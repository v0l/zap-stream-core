FROM voidic/rust-ffmpeg AS build
WORKDIR /app/src
COPY . .
ENV LD_LIBRARY_PATH=$FFMPEG_DIR/lib
ARG CARGO_FEATURES=""
RUN cargo test ${CARGO_FEATURES}
RUN cargo install --path ./crates/zap-stream --root /app/build ${CARGO_FEATURES}

FROM rust:bookworm
WORKDIR /app
RUN apt update && \
    apt install -y libx264-164 && \
    rm -rf /var/lib/apt/lists/*
COPY --from=build /app/build .
COPY --from=build /app/src/ffmpeg/lib/ /lib
ENTRYPOINT ["/app/bin/zap-stream"]