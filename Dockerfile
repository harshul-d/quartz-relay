# Click-stream sessionization pipeline: one statically linked Rust binary.
# Build stage: the official Rust image (glibc static libraries included).
FROM rust:1 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Static glibc binary for the scorer's Xeons (Cascade Lake or newer, so
# x86-64-v3 is always available). --target keeps the static flag off any
# build scripts.
ENV RUSTFLAGS="-C target-feature=+crt-static -C target-cpu=x86-64-v3"
RUN cargo build --release --locked --target x86_64-unknown-linux-gnu \
 && cp target/x86_64-unknown-linux-gnu/release/pipeline /pipeline

# Run stage: a minimal Debian. The binary needs nothing from it.
FROM debian:stable-slim
COPY --from=build /pipeline /usr/local/bin/pipeline
# The scorer runs as uid 1000 with a read-only root, no network, /data
# read-only and /output writable. Paths come from DATA_DIR and OUTPUT_DIR.
USER 1000:1000
ENV DATA_DIR=/data OUTPUT_DIR=/output SCRATCH_DIR=/scratch
CMD ["/usr/local/bin/pipeline"]
