# #74 응시 앱 화면 v2 스크린샷

리눅스(Docker 컨테이너 안 Xvfb + openbox, WebKitGTK)에서 **실제 응시 앱**(`promptlab-app` release 빌드, 워크트리 그대로 — 79e155b)을 화면 크기 1920×1080·1280×800으로 띄워 xdotool로 눌러 가며 찍었다(창은 화면 가득, 창 틀 없음). 로컬 모델은 실제 Qwen2.5-1.5B(대화 턴과 공개 예시 시험 결과가 실제 모델 출력)이고, 서버는 앱이 부르는 경로만 흉내 낸 가짜 API(배정 목록·문항 시작·과정 로그 받기·제출·채점 결과가 고정값 — 운영 auth·DB·워커·Claude 없음, 토큰은 가짜 문자열 `PROMPTLAB_ACCESS_TOKEN`)다. 다크는 `GTK_THEME=Adwaita:dark`(웹뷰 `prefers-color-scheme: dark`). 1280은 폭이 1440 미만이라 왼쪽 메뉴와 응시 중 문제 칸이 접힌 배치가 저절로 나온다.

**글꼴 주의**: 컨테이너에는 Segoe UI Variable·맑은 고딕이 없어 `system-ui`(DejaVu Sans)와 Noto CJK로 보인다 — Windows에서는 Segoe UI Variable/맑은 고딕으로 보인다.

**우회한 것**:
- 웹캠이 없다. 점검 화면(`check_*`)은 웹캠 동의·기준 사진이 필요한 배정(채용 시험)을 골라 「웹캠 아직 확인 전」 상태로 찍었고, 응시 화면으로는 가짜 API가 「이미 동의하고 기준 사진을 올린」 배정(사내 진단)을 줘서 넘어갔다(점검 화면에서 웹캠 단계 없이 「기준 사진을 이미 올렸다 — 이어서 응시한다」). 제출 → 채점 대기 → 결과는 앱 흐름 그대로 이어서 찍었다.
- 가로 넘침은 마우스를 먼저 옮겨 두고 오른쪽으로 15칸 굴린 뒤, 위 머리띠(남은 시간)를 뺀 화면이 바뀌는지(`compare -metric AE`, 바뀐 픽셀 수)로 쟀다: Main·Check·연습(1920·1280), Exam·Submit·Result(1280) 모두 0(점검 1280은 세로 스크롤 막대가 잠깐 뜬 것만 바뀜). 1920 Exam·Submit은 따로 재지 않았고 사진에 가로 스크롤 막대가 없다.
- 채점 결과 수치(숨은 케이스 6/8, 대화 평가 항목·근거, 기준선 대비, 소요 412초)는 가짜 API의 고정값이다.

| 파일 | 화면 |
|---|---|
| `main_1920_light`·`main_1920_dark`·`main_1280_light`·`main_1280_dark` | Main — 내 시험 목록(응시 가능·끝난 시험) + 상세(시험 방식·점수 구성·문항) |
| `check_1920_light`·`check_1920_dark`·`check_1280_light`·`check_1280_dark` | Check — 응시 전 점검·동의(웹캠 확인 전, 채용 시험) |
| `exam_1920_light`·`exam_1920_dark` | Exam — 넓은 3칸(문제 \| 대화 \| 점검·제출), 대화 A 2턴 + 대화 B 1턴, 대화 A 공개 예시 3/3 통과 |
| `exam_1280_light`·`exam_1280_dark` | Exam — 좁은 배치(문제 칸이 48px 막대로 접힘) |
| `exam_1280_light_drawer` | Exam — 좁은 배치에서 문제 막대를 눌러 연 서랍 |
| `submit_1920_light`·`submit_1920_dark`·`submit_1280_light`·`submit_1280_dark` | Submit — 제출 ContentDialog |
| `result_1920_light`·`result_1920_dark`·`result_1280_light`·`result_1280_dark` | Result — 최종 점수·계산식·기준선 대비·과정, 문항 \| 숨은 케이스·대화 평가 \| 대화 보기 3칸 |
| `practice_1920_light` | Main(연습) — 개인 연습 문제 목록 + 상세 |
