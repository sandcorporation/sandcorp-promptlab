# 판정기용 docker 소켓 프록시(promptlab-docker-proxy, #95) 이미지 — bin 하나. docker 소켓을 마운트하는 유일한 서비스다.
# 빌드 문맥은 저장소 루트. arm64(oracle)·amd64 둘 다 buildx 로 굽는다.
#
# 권한: 소켓(root:docker 660)의 docker 그룹 gid 는 호스트마다 달라 이미지·compose 에 박지 않는다. 그래서 root 로 시작해
# 소켓 파일의 gid 를 읽고 보조 그룹으로 둔 뒤 uid·gid 65534(nobody)로 내려앉는다(crates/docker-proxy/src/privileges.rs).
# compose 는 cap_drop: [ALL] + cap_add: [SETUID, SETGID] 만 준다 — uid 를 바꾸는 순간 커널이 그 둘도 거둔다.
# 그래서 USER 를 두지 않는다(root 가 아닌 채로 띄우면 내리기를 건너뛰고 그 uid 그대로 돈다).
ARG RUST_VERSION=1.98.1
FROM rust:${RUST_VERSION}-slim-bookworm AS build
ARG RUST_VERSION
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} CARGO_TARGET_DIR=/target CARGO_TERM_COLOR=never
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=promptlab-cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=promptlab-docker-proxy-target,target=/target \
    cargo build --release --locked -p promptlab-docker-proxy \
 && mkdir -p /out \
 && cp /target/release/promptlab-docker-proxy /out/

FROM debian:bookworm-slim
COPY --from=build /out/promptlab-docker-proxy /usr/local/bin/promptlab-docker-proxy
ENV PROMPTLAB_PROXY_ADDR=0.0.0.0:2375 PROMPTLAB_DOCKER_SOCKET=/var/run/docker.sock
EXPOSE 2375
CMD ["promptlab-docker-proxy"]
