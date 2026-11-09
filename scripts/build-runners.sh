#!/usr/bin/env bash
# 러너 이미지 4개(docker/runners/<언어>.Dockerfile)를 굽는다 — spec #93 2절.
#
#   bash scripts/build-runners.sh [--local] [언어 ...]      언어: python javascript java cpp (비면 넷 다)
#
# 기본: buildx 로 linux/amd64 + linux/arm64 를 함께 굽고 ghcr.io/sandcorporation/promptlab-runner-<언어>:<판> 에 push 한 뒤
#       멀티아키 매니페스트 digest 를 찍는다. 그 digest 를 crates/shared/src/coding/mod.rs 의 RUNNER_IMAGES 에 적는다.
# --local: 이 기계 아키텍처 하나만 굽고 로컬 docker 에 싣는다(push 없음) — 판정기 docker 시험(`--ignored`)용.
#
# 판(태그)은 crates/shared/src/coding/mod.rs 의 RUNNER_TAG 한 곳에서 읽는다. 이미 올린 판을 다시 굽지 않는다 —
# 이미지를 바꾸면 RUNNER_TAG 를 올린다(digest 고정이라 같은 태그를 덮어써도 앱·판정기는 옛 digest 를 받는다).
#
# 환경변수
#   GHCR_TOKEN    ghcr 쓰기 토큰(push 때). 노드 docker 설정은 건드리지 않고 임시 DOCKER_CONFIG 에만 적는다. 없으면 노드 로그인 그대로
#   BUILDER_NAME  buildx 빌더(docker-container) 이름, 기본 promptlab-runners
#   PLATFORMS     기본 linux/amd64,linux/arm64
#   DIGEST_FILE   digest 를 「언어 digest」 줄로 적을 파일(선택, Jenkins 아카이브용)
# 토큰 값은 찍지 않는다.
set -euo pipefail

cd "$(dirname "$0")/.."

local_only=0
langs=()
for a in "$@"; do
  case "$a" in
    --local) local_only=1 ;;
    python|javascript|java|cpp) langs+=("$a") ;;
    *) echo "모르는 인자: $a" >&2; exit 2 ;;
  esac
done
if [ ${#langs[@]} -eq 0 ]; then langs=(python javascript java cpp); fi

tag="$(sed -n 's/^pub const RUNNER_TAG: &str = "\([^"]*\)";/\1/p' crates/shared/src/coding/mod.rs)"
if [ -z "$tag" ]; then echo "RUNNER_TAG 를 찾지 못했다" >&2; exit 1; fi
repo_base="ghcr.io/sandcorporation/promptlab-runner"
rev="$(git rev-parse HEAD 2>/dev/null || echo unknown)"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/promptlab-runners.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

if [ "$local_only" = 1 ]; then
  for l in "${langs[@]}"; do
    echo "== $l (로컬, $repo_base-$l:$tag)"
    docker build --file "docker/runners/$l.Dockerfile" --tag "$repo_base-$l:$tag" \
      --label "org.opencontainers.image.revision=$rev" .
  done
  exit 0
fi

if [ -n "${GHCR_TOKEN:-}" ]; then
  orig="${DOCKER_CONFIG:-$HOME/.docker}"
  mkdir -p "$orig/buildx"
  for d in cli-plugins buildx contexts; do
    if [ -e "$orig/$d" ]; then ln -s "$orig/$d" "$tmp/$d"; fi
  done
  python3 - "$orig/config.json" "$tmp/config.json" <<'PY'
import base64, json, os, sys
src, dst = sys.argv[1:3]
try:
    with open(src) as f:
        cfg = json.load(f)
except (OSError, ValueError):
    cfg = {}
for key in ("auths", "credsStore", "credHelpers"):
    cfg.pop(key, None)
tok = os.environ["GHCR_TOKEN"].strip()
cfg["auths"] = {"ghcr.io": {"auth": base64.b64encode(f"sandcorp-ci:{tok}".encode()).decode()}}
fd = os.open(dst, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as f:
    json.dump(cfg, f)
PY
  export DOCKER_CONFIG="$tmp"
else
  echo "GHCR_TOKEN 이 없다 — 이 노드의 docker 로그인을 그대로 쓴다"
fi

builder="${BUILDER_NAME:-promptlab-runners}"
platforms="${PLATFORMS:-linux/amd64,linux/arm64}"
if ! docker buildx inspect "$builder" >/dev/null 2>&1; then
  echo "빌더 $builder 를 만든다(docker-container)"
  docker buildx create --name "$builder" --driver docker-container >/dev/null
fi
docker buildx inspect --bootstrap "$builder" >/dev/null

results=()
for l in "${langs[@]}"; do
  image="$repo_base-$l"
  echo "== $l ($image:$tag, $platforms)"
  docker buildx build --builder "$builder" --platform "$platforms" --provenance=false --push \
    --file "docker/runners/$l.Dockerfile" \
    --tag "$image:$tag" \
    --label "org.opencontainers.image.revision=$rev" \
    --label "org.opencontainers.image.source=https://github.com/sandcorporation/sandcorp-promptlab" \
    --label "org.opencontainers.image.description=promptlab 코딩 문제 러너($l)" \
    --cache-from "type=registry,ref=$image-buildcache" \
    --cache-to "type=registry,ref=$image-buildcache,mode=max" \
    --metadata-file "$tmp/$l.json" \
    .
  digest="$(sed -n 's/.*"containerimage\.digest": *"\(sha256:[0-9a-f]*\)".*/\1/p' "$tmp/$l.json" | head -n 1)"
  if [ -z "$digest" ]; then echo "$l digest 를 읽지 못했다" >&2; exit 1; fi
  results+=("$l $digest")
done

echo
echo "== digest ($tag) — crates/shared/src/coding/mod.rs RUNNER_IMAGES 에 적는다"
for r in "${results[@]}"; do echo "$r"; done
if [ -n "${DIGEST_FILE:-}" ]; then printf '%s\n' "${results[@]}" > "$DIGEST_FILE"; fi
