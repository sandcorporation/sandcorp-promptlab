#!/usr/bin/env bash
# sandcorp-design(github.com/sandcorporation/sandcorp-design)을 태그 고정으로 design/ 에 값 복사한다(#41).
# 사용: bash scripts/sync-design.sh v0.7.0
# 복사본은 커밋하고 손으로 고치지 않는다. 올릴 때는 태그만 바꿔 다시 돌린다.
# 대시보드(wasm)·응시 앱(데스크톱)이 design/sandcorp.css 를 include_str! 로 바이너리에 굽고,
# 서버 이미지가 design/ 의 svg 를 /design/ 에서 서빙한다(파비콘).
# 비공개 저장소라 gh(로그인된 GitHub CLI)로 받는다.
set -euo pipefail
TAG="${1:?태그 필요 (예: v0.7.0)}"
REPO="sandcorporation/sandcorp-design"
DEST="$(cd "$(dirname "$0")/.." && pwd)/design"
mkdir -p "$DEST"
fetch() { # <저장소 안 경로> <저장할 이름>
  gh api -H 'Accept: application/vnd.github.raw' "repos/${REPO}/contents/$1?ref=${TAG}" > "${DEST}/$2.tmp"
  mv "${DEST}/$2.tmp" "${DEST}/$2"
}
fetch dist/sandcorp.css sandcorp.css
for f in logo.svg logo-dark.svg mark.svg mark-dark.svg; do fetch "assets/$f" "$f"; done
echo "${TAG}" > "${DEST}/VERSION"
echo "design/ ← ${REPO}@${TAG}"
