FROM lukemathwalker/cargo-chef:latest-rust-trixie AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
RUN apt-get update -y \
  && apt-get install -y --no-install-recommends \
  pkg-config \
  cmake \
  nasm \
  libdav1d-dev \
  libheif-dev \
  libjxl-dev \
  && apt-get clean -y \
  && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin previewproxy

FROM debian:trixie-slim AS pdfium
ARG PDFIUM_VERSION=7749
ARG TARGETARCH
RUN apt-get update -y \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && apt-get clean -y && rm -rf /var/lib/apt/lists/*
RUN case "$TARGETARCH" in \
  amd64) PDFIUM_ARCH="x64" ;; \
  arm64) PDFIUM_ARCH="arm64" ;; \
  386)   PDFIUM_ARCH="x86" ;; \
  *)     echo "Unsupported arch: $TARGETARCH" && exit 1 ;; \
  esac \
  && curl -L \
  "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${PDFIUM_VERSION}/pdfium-linux-${PDFIUM_ARCH}.tgz" \
  | tar xz -C /usr/local

FROM debian:trixie-slim AS runtime
WORKDIR /app
RUN apt-get update -y \
  && apt-get install -y --no-install-recommends \
  ca-certificates \
  curl \
  libdav1d7 \
  libheif1 \
  ffmpeg \
  && apt-get clean -y \
  && rm -rf /var/lib/apt/lists/*
COPY --from=pdfium /usr/local/lib /usr/local/lib
RUN ldconfig
COPY --from=builder /app/target/release/previewproxy previewproxy
ENV PORT=8080
ENV APP_ENV=production
ENV RUST_LOG="previewproxy=info,tower_http=info"
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -f http://localhost:8080/health || exit 1
ENTRYPOINT ["./previewproxy"]
