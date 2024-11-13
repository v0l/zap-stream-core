ARG IMAGE=rust:bookworm

FROM $IMAGE as build
WORKDIR /app/src
ENV FFMPEG_DIR=/app/ffmpeg
COPY . .
RUN apt update && \
    apt install -y \
    build-essential \
    libx264-dev \
    nasm \
    libclang-dev && \
    rm -rf /var/lib/apt/lists/*
RUN git clone --depth=1 https://git.ffmpeg.org/ffmpeg.git && \
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
    --disable-static \
    --enable-shared && \
    make -j$(nproc) && make install
RUN cargo install --path . --bin zap-stream-core --root /app/build

FROM $IMAGE as runner
WORKDIR /app
RUN apt update && \
    apt install -y libx264-164 && \
    rm -rf /var/lib/apt/lists/*
COPY --from=build /app/build .
COPY --from=build /app/ffmpeg/lib/ /lib
ENTRYPOINT ["/app/bin/stream-core"]