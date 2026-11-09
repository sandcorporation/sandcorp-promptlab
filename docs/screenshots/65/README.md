# #65 응시 앱 Windows 11 Fluent 스타일 스크린샷

리눅스(Docker 컨테이너 안 Xvfb 1280×800, WebKitGTK)에서 **실제 응시 앱**(`promptlab-app` 디버그 빌드)을 띄워 xdotool로 눌러 가며 찍었다. 로컬 모델은 실제 Qwen2.5-1.5B(공개 예시 시험 출력이 실제 모델 출력)이고, 서버는 앱이 부르는 경로만 흉내 낸 가짜 API(채점 결과·배정 목록은 고정값 — 운영 auth·DB·워커 없음, 토큰은 가짜 문자열)다. 다크는 `GTK_THEME=Adwaita:dark`(웹뷰 `prefers-color-scheme: dark`).

**글꼴 주의**: 컨테이너에는 Segoe UI Variable·맑은 고딕이 없어 `system-ui`(DejaVu Sans)와 Noto CJK로 보인다 — Windows에서는 Segoe UI Variable/맑은 고딕으로 보인다. 시작 화면 사진(`m_*`·`p_light`·`s_*`)은 #67에서 Dioxus 기본 메뉴 막대(Window·Edit·Help)를 끈 뒤 다시 찍었다. 응시 중·점검·결과 화면(`c*`·`e_*`·`r_*`·`done_light`)은 모델·웹캠 흐름이 필요해 #65 때 사진 그대로이고, 맨 위 줄(Window·Edit·Help)은 옛 기본 메뉴라 지금 앱에는 없다. 웹캠이 없어 점검 화면은 「웹캠 아직 확인 전」 상태다. 「과정 로그 미전송」 배지는 가짜 서버에 로그 경로가 없어서 뜬 것이다.

| 파일 | 시안 | 화면 |
|---|---|---|
| `m_light`·`m_dark` | Main | 내 시험(배정 목록: 시작 전·응시 중·응시 완료 배지) |
| `p_light` | Main(연습) | 개인 연습 문제 목록 |
| `s_light`·`s_dark` | — | 설정(정보 표시만) |
| `c_light`·`c2_light`·`c_dark` | Check | 응시 전 점검·동의 |
| `e_light`·`e_dark` | Exam | 응시 중(전체화면 잠금 중 배지 — 배치는 그대로, 색·글꼴·컨트롤만) |
| `r_light`·`r_dark` | Result | 채점 결과(개인 연습) |
| `done_light` | Result | 시험 응시 완료(채용은 점수 대신) |

시안과 다른 점: 시안의 직접 그린 제목 막대(최소화·최대화·닫기)는 OS 창 틀이 대신하므로 그리지 않았다. 계정 항목(이름·카카오)은 앱이 사용자 이름을 알지 못해 두지 않고 설정에 로그인 안내만 적었다. 시안 Main의 「마감 날짜」·「남은 시간」은 배정 API에 없어 넣지 않았다. 점검 화면은 시안의 2열(웹캠 | 점검)이 아니라 세로 한 줄 흐름이다.
