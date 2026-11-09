#!/usr/bin/env bash
# 응시 앱 편집기(Monaco, #97) 자산을 고정 판으로 받아 crates/app/assets/monaco/에 복사한다.
#
# - 판·sha256은 아래 상수 한 곳. 올릴 때는 두 값을 바꿔 다시 돌리고 결과를 커밋한다(손으로 고치지 않는다).
# - npm 꾸러미(min AMD 빌드)에서 응시 앱이 쓰는 것만 옮긴다: 로더·편집기 본체·CSS·아이콘 글꼴·한국어 문구·
#   4개 언어(Python·JavaScript·Java·C++) 구문 강조·worker 진입점, 그리고 라이선스(MIT)·제3자 고지.
# - 앱은 이 파일들을 include_bytes!로 바이너리에 굽고 웹뷰에 직접 내준다(외부 요청·CDN 없음, crates/app/src/editor_assets.rs).
#
# 쓰임: bash scripts/fetch-monaco.sh
set -euo pipefail

VERSION=0.52.2
SHA256=c280cdcf0b0c13d1a2bf01af958d4387ed06d7f6c918401d00c4adcae1bc72b6
URL="https://registry.npmjs.org/monaco-editor/-/monaco-editor-${VERSION}.tgz"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/crates/app/assets/monaco"
FILES=(
  vs/loader.js
  vs/nls.messages.ko.js
  vs/editor/editor.main.js
  vs/editor/editor.main.css
  vs/base/browser/ui/codicons/codicon/codicon.ttf
  vs/base/worker/workerMain.js
  vs/basic-languages/python/python.js
  vs/basic-languages/javascript/javascript.js
  vs/basic-languages/java/java.js
  vs/basic-languages/cpp/cpp.js
)

WORK="$(mktemp -d)"
trap 'rm -r "$WORK"' EXIT
curl -sfL "$URL" -o "$WORK/monaco.tgz"
echo "$SHA256  $WORK/monaco.tgz" | sha256sum -c --quiet -
tar xzf "$WORK/monaco.tgz" -C "$WORK"

mkdir -p "$DEST"
find "$DEST" -mindepth 1 -delete
for f in "${FILES[@]}"; do
  mkdir -p "$DEST/$(dirname "$f")"
  cp "$WORK/package/min/$f" "$DEST/$f"
done
cp "$WORK/package/LICENSE" "$DEST/LICENSE"
cp "$WORK/package/ThirdPartyNotices.txt" "$DEST/ThirdPartyNotices.txt"
echo "$VERSION" > "$DEST/VERSION"
echo "monaco-editor $VERSION → $DEST ($(du -sh "$DEST" | cut -f1))"
