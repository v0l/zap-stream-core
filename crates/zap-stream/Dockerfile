ARG IMAGE=rust:bookworm

FROM $IMAGE AS build
WORKDIR /app/src
ENV FFMPEG_DIR=/app/ffmpeg
COPY . .
RUN apt update && \
    apt install -y \
    build-essential \
    libx264-dev \
    libx265-dev \
    libwebp-dev \
    libpng-dev \
    nasm \
    protobuf-compiler \
    libclang-dev && \
    rm -rf /var/lib/apt/lists/*
RUN git clone --single-branch --branch release/7.1 https://git.ffmpeg.org/ffmpeg.git && \
    cd ffmpeg && \
    ./configure \
    --prefix=$FFMPEG_DIR \
    --disable-programs \
    --disable-doc \
    --disable-network \
    --enable-gpl \
    --enable-version3 \
    --disable-postproc \
    --enable-libx264 \
    --enable-libx265 \
    --enable-libwebp \
    --disable-static \
    --enable-shared && \
    make -j$(nproc) && make install
RUN cargo install --path ./crates/zap-stream --root /app/build

FROM $IMAGE AS runner
WORKDIR /app
RUN apt update && \
    apt install -y libx264-164 && \
    rm -rf /var/lib/apt/lists/*
COPY --from=build /app/build .
COPY --from=build /app/ffmpeg/lib/ /lib
ENTRYPOINT ["/app/bin/zap-stream"]