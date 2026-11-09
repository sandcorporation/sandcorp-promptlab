# 판정기(promptlab-judge) 이미지 — judge bin + docker CLI(정적 바이너리)만. compose 내부 네트워크에서 POST /run 을 받고,
# docker CLI 는 DOCKER_HOST=tcp://docker-proxy:2375(본문 검사 소켓 프록시, #95)로만 데몬에 닿는다 — 소켓은 마운트하지 않는다.
# 빌드 문맥은 저장소 루트. arm64(oracle)·amd64 둘 다 buildx 로 굽는다(rust·debian·docker:cli 모두 멀티아키).
ARG RUST_VERSION=1.98.1

# docker CLI 판은 소켓 프록시 시험 픽스처(crates/docker-proxy/tests/fixtures — 이 CLI 가 보내는 만들기 본문)와 같아야 한다.
# 올리면 픽스처를 다시 잡는다(README 참고). 태그 + 멀티아키 목록 digest 로 고정.
FROM docker:29.8.1-cli@sha256:018edbc908e08fcc9dbf029c812c34251e9b4719e6f71ca0e5eae2a987d014ca AS dockercli

FROM rust:${RUST_VERSION}-slim-bookworm AS build
ARG RUST_VERSION
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} CARGO_TARGET_DIR=/target CARGO_TERM_COLOR=never
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=promptlab-cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=promptlab-judge-target,target=/target \
    cargo build --release --locked -p promptlab-judge --bin promptlab-judge \
 && mkdir -p /out \
 && cp /target/release/promptlab-judge /out/

FROM debian:bookworm-slim
COPY --from=dockercli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=build /out/promptlab-judge /usr/local/bin/promptlab-judge
# 루트 아님(nobody). docker CLI 설정은 쓰기 가능한 tmpfs(/tmp, compose)에 — 루트 파일시스템은 읽기 전용으로 띄운다.
USER 65534:65534
ENV PROMPTLAB_JUDGE_ADDR=0.0.0.0:8090 HOME=/tmp DOCKER_CONFIG=/tmp/.docker
EXPOSE 8090
CMD ["promptlab-judge"]
