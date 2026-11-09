# PR 검사·로컬 검사용 이미지. scripts/ci/check.sh 가 굽고 그 안에서 cargo 를 돌린다.
# RUST_VERSION 은 check.sh 가 rust-toolchain.toml 의 channel 에서 읽어 넘긴다(버전의 정본은 그 파일).
ARG RUST_VERSION
FROM rust:${RUST_VERSION}-slim-bookworm

# Dioxus 데스크톱(wry/tao: webkit2gtk·gtk3·xdo)을 리눅스에서 빌드하는 데 필요한 시스템 패키지.
# cmake·clang·libclang: llama.cpp(llama-cpp-sys-2) 빌드와 bindgen. curl: scripts/fetch-model.sh.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config libssl-dev \
      libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
      libxdo-dev librsvg2-dev \
      cmake clang libclang-dev curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy \
 && rustup target add wasm32-unknown-unknown

ENV CARGO_TERM_COLOR=never \
    CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR=/target
WORKDIR /src
