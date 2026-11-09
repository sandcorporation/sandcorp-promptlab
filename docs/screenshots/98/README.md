# #98 응시 앱 AI 도우미 스크린샷

#97과 같은 방식: 리눅스 Docker 컨테이너 안 Xvfb + openbox, WebKitGTK에서 **실제 응시 앱**(`promptlab-app` release, 브랜치 t98)을 1920×1080·1280×800으로 띄워 xdotool로 눌러 가며 찍었다. 다크는 `GTK_THEME=Adwaita:dark`(웹뷰 `prefers-color-scheme: dark`, Monaco·diff는 `vs-dark`). 서버는 앱이 부르는 경로만 흉내 낸 가짜 API(사내 진단 배정, 시험 문항 하나의 공개 보기), Docker는 호스트 데몬.

- **AI는 실제 모델**: `Qwen2.5-Coder-7B-Instruct` Q4_K_M(4.7GB, 고정 sha256)을 컨테이너에 읽기 전용으로 넣고 **CPU**(12스레드, GPU 없음 — 점검·머리의 「CPU」 배지)로 돌렸다. 대화·도구 표시·제안 diff는 모델이 실제로 낸 것이다.
- 응시자는 `return dp[amount]`(못 만들면 -1을 돌려주지 않는 버그) 코드를 치고 AI에게 물었다. 7B가 CPU·탐욕적 선택에서 낸 제안은 **틀린 편집**이다(들여쓰기 없이 7번째 줄을 베낌) — 화면은 제안·diff·수락 흐름을 보이려는 것이고, 받아들일지는 응시자가 정한다(수락하면 그대로 들어가고 Ctrl+Z 한 번으로 되돌린다). 과정 로그에는 `ai_message`·`ai_reply`·`edit_proposed`·`edit_accepted`가 올라갔다(가짜 API 로그로 확인).
- 글꼴: 컨테이너에 Segoe UI Variable·맑은 고딕·Consolas가 없어 대체 글꼴로 보인다(#97 README와 같음).

| 파일 | 화면 |
|---|---|
| `check_<폭>_<테마>` | 응시 전 점검 — 장치 카드에 새 행 「AI 속도: CPU」(GPU(Vulkan) 없음)·「메모리 27GB — 권장 16GB 이상」 |
| `thinking_1920_light` | 질문을 보내고 모델이 답하는 중(「AI가 생각하는 중…」) |
| `diff_1920_light` | 첫 질문에 모델은 `read_problem`·`read_code` 뒤 글로만 답했고(코드 블록), 「propose_edit 도구로 고쳐 제안해 주세요」라고 다시 묻자 `propose_edit`를 냈다 → 편집기 자리 위 Monaco diff 덮개(「AI 제안 #1」·8–8줄·수락·거절·닫기)와 AI 칸 제안 카드(결정 대기·diff 보기·수락·거절) |
| `accepted_1920_light` | 수락 뒤 — 덮개가 닫히고 8번째 줄만 바뀐 편집기, 카드는 「수락함」 |
| `fabricated_1280_light`·`fabricated_1920_dark`·`fabricated_1280_dark` | **모델이 제안을 내지 못한 설정**: 같은 질문 한 번에 7B가 `read_code` 뒤 `propose_edit`를 부르지 않고 도구 결과(`<tool_response>` + 고친 코드)를 스스로 지어 냈다. 이 화면은 그 습관을 바로잡기 **전** 빌드다 — 그 뒤 앱은 도구 호출 없는 답에 `<tool_response>`가 있으면 화면에 보이지 않고 한 번 바로잡아 다시 묻는다(`shared::ai::FABRICATED_RESULT_NOTE`, 가짜 추론 시험 `도구_결과를_지어_낸_답은_한_번_바로잡아_다시_묻는다`). 바로잡기 뒤 빌드로는 다시 찍지 못했다(조율자 지시로 PR 먼저) |

다크·1280의 diff·수락 뒤 화면은 없다(위 이유). 덮개·카드는 토큰만 써서 다크에서도 같은 구조다.
