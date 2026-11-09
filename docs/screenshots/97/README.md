# #97 응시 앱 코딩 응시 화면 스크린샷

리눅스(Docker 컨테이너 안 Xvfb + openbox, WebKitGTK)에서 **실제 응시 앱**(`promptlab-app` debug 빌드, 워크트리 t97 그대로)을 화면 크기 1920×1080·1280×800으로 띄워 xdotool로 눌러 가며 찍었다(창은 화면 가득, 창 틀 없음). 다크는 `GTK_THEME=Adwaita:dark`(웹뷰 `prefers-color-scheme: dark`, Monaco는 `vs-dark`). 1280은 폭이 1440 미만이라 왼쪽 메뉴와 응시 중 문제 칸이 접힌 배치가 저절로 나온다.

- **서버**: 앱이 부르는 경로만 흉내 낸 가짜 API(파이썬). 배정 목록(사내 진단 = 웹캠 동의·기준 사진 끝, 허용 언어 Python·Java·C++ / 채용 = 웹캠 필요 / 끝난 시험 하나), 문항 시작, 과정 로그 키·업로드, 실시간 보기 폴링, 제출, 채점 결과(숨은 케이스 6/8 고정값)를 200으로 답한다. 문제는 내장 문항의 **공개 보기**(`CodingProblem::public_view()`로 뽑은 JSON — 시험 문항 하나, 연습 4문항)라 서명·예제·4개 언어 틀이 shared와 같다. 운영 auth·DB·워커·판정기·Claude 없음, 토큰은 가짜 문자열(`PROMPTLAB_ACCESS_TOKEN`).
- **Docker는 실제**: 컨테이너에 호스트 docker CLI와 `/var/run/docker.sock`을 넣어 응시 전 점검(엔진 확인 → 허용 언어 러너 이미지 확인 → 빈 틀 한 번 실행)·「실행」·「예제 테스트」가 호스트 docker 데몬의 고정 digest 러너 이미지로 실제로 돈다. 표의 반환값·표준 출력·시간은 실제 실행 결과다.
- **모델 파일**: 실제 Qwen2.5-1.5B GGUF(읽기 전용) — 문항 시작이 sha256을 신고한다(가짜 API는 대조하지 않는다).

**글꼴 주의**: 컨테이너에는 Segoe UI Variable·맑은 고딕·Consolas가 없어 `system-ui`(DejaVu Sans)·Noto CJK·DejaVu Sans Mono로 보인다(일부 굵은 숫자·제목이 세리프처럼 보이는 것도 이 대체 글꼴 때문) — Windows에서는 Segoe UI Variable/맑은 고딕/Consolas로 보인다.

**우회한 것**:
- 웹캠이 없다. 가짜 API가 「이미 동의하고 기준 사진을 올린」 사내 진단 배정을 줘서 점검 화면의 웹캠 단계 없이 응시로 넘어갔다(응시 중 웹캠은 `camera_unavailable` 사건만 남는다).
- 코드는 xdotool로 Monaco에 한 줄씩 쳤다(자동 들여쓰기에 맡기고 마지막 줄만 Shift+Tab으로 내어 씀).
- `check_nodocker_1920_light`는 docker CLI를 PATH에 넣지 않은 컨테이너다(소켓도 없음). 동의 칸을 켜도 「응시 시작」이 잠긴 채이고 문항 시작 요청이 나가지 않는다.
- `paste_1920_light`: 앱 안 복사 버퍼가 빈 채로 `xclip`으로 OS 클립보드에 밖의 글(62자)을 넣고 편집기에서 Ctrl+V — 편집기에 들어가지 않고 오른쪽 칸 InfoBar가 뜨며, 가짜 API가 받은 과정 로그에 `{"type":"paste_blocked","chars":62}`가 왔다. (앱 안에서 Ctrl+C로 복사한 줄은 Ctrl+V로 들어가고, 그때 OS 클립보드는 비어 있다 — 앱이 OS 클립보드에 쓰지 않는다.)
- 가로 넘침은 마우스를 먼저 옮겨 두고 오른쪽으로 15칸 굴린 뒤, 위 머리띠(52px)를 뺀 화면이 바뀌는지(`compare -metric AE`)로 쟀다: 점검·응시(편집기·실행 칸) 1920·1280 모두 내용이 옆으로 밀리지 않았다(0이 아닌 값은 굴릴 때 잠깐 뜨는 세로 스크롤 막대뿐 — 차이 그림으로 확인).

| 파일 | 화면 |
|---|---|
| `check_1920_light`·`check_1920_dark`·`check_1280_light`·`check_1280_dark` | 응시 전 점검 — 사내 진단, Docker 카드(엔진 + 허용 언어 3개 모두 준비됨), 아래 고정 막대. 1280은 Docker 카드가 보이게 조금 굴렸다 |
| `check_nodocker_1920_light` | 점검 — Docker 없음: 「안 됨」, 설치 안내 단추, 요금 안내 문장, 동의해도 「응시 시작」 잠김 |
| `exam_1920_light`·`exam_1920_dark` | 응시 중 3칸(문제 \| Monaco + 실행 칸 \| AI 자리) — 정답 코드, 「예제 테스트」 3/3 통과 표 |
| `exam_1280_light`·`exam_1280_dark` | 응시 중 좁은 배치(문제 칸이 48px 막대로 접힘) — 같은 상태 |
| `run_1920_light` | 「실행」 — 입력 칸 인자 `[[1,5,10,25],63]`, 반환값 6과 표준 출력 |
| `paste_1920_light` | 밖에서 복사한 글 붙여넣기 막음 — 오른쪽 칸 InfoBar |
| `submit_1920_light` | 제출 확인 상자(언어·코드 바이트·마지막 예제 테스트) |
