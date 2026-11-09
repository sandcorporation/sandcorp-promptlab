# #100 대시보드 새 화면 스크린샷 (응시자 상세 코드·편집 타임라인·AI 대화·근거 칩, 약관·삭제 상자 수정)

브랜치 `t100`(커밋 `2667052`, 결함 수정 뒤 다시 찍음)의 서버 이미지(`docker/server.Dockerfile`, wasm 대시보드 포함)를 빌드하고 Postgres 17 컨테이너와 같은 도커 네트워크에 띄웠다(`t100-` 접두). 시드는 SQL 직접, 로그인은 `sessions`에 세션 쿠키 `pl_session` 값의 sha256을 직접 넣었다. 운영 auth·실제 Anthropic API 없음(워커·판정기·키 없음 — 채점·평가 결과도 SQL로 직접 넣었다).

시드: 조직 1(관리자·감독관 구성원, 약관 동의 기록 2건), 채용 시험 1(시험 문항 2개, 점수 비율 60:40), 응시자 3명.
- ① 채점 완료 + AI 활용 평가 완료. 문항 1 히든 10/11·평가 85.0(근거 칩 「턴 1~3」「제안 #1」「제안 #2」), 문항 2 8/13·평가 평균 이하(AI 미사용, 근거 없음).
- ② 응시 중(자바스크립트, 로그 9건). ③ 초대만.
- ①·②의 과정 로그는 `promptlab_shared::log::LogSigner::seal`로 만든 서명·해시 체인 사건(LOG_FORMAT 2): Started·LanguageChosen·CodeSnapshot 5회·AiMessage/AiReply(도구 호출 `read_code`·`propose_edit`)·EditProposed #1(수락, 스냅숏에서 `best = 0` 줄을 고쳐 「수락 뒤 수정」)·EditProposed #2(거절)·LocalRun 3·PasteBlocked·FocusLost/Returned(18초)·Submitted. 서버가 다시 검증해도 `Tampered` 신호 없음(의심 신호는 붙여넣기 막음·포커스 이탈 2건만 나왔다). 웹캠 사진은 시드하지 않아 웹캠 탭은 찍지 않았다.
- 별도 사용자(조직 없음)로 `create-org`·`invite-accept`(초대 토큰 해시 시드).

Playwright(헤드리스 크롬, `colorScheme` 라이트·다크) 1440x900 / 390x844 전체 페이지, JPEG 품질 55. 파일 이름 `<화면>-<desktop|mobile>-<light|dark>.jpg`. 이 컨테이너 크롬에는 한글 글꼴이 WenQuanYi 하나뿐이라 글꼴 모양·폭은 실제 Windows·macOS와 다르다(줄바꿈 위치는 달라질 수 있다).

| 화면 | 주소(해시) | 보이는 것 |
|---|---|---|
| `exam-results` | `#/orgs/<org>/exams/<exam>/results` | 요약 칸·상태 칩·응시자 표 |
| `candidate-score` | `…/candidates/<invite>/conversations` | 제출 코드·편집 타임라인(「줄 변경 보기」·제안 글 전부 펼침)·AI 대화 원문·로컬 실행·점수 계산식·평가 항목 |
| `candidate-evidence-click` | 같은 주소 | 「제안 #2」 칩 클릭 뒤(전체 페이지). 뷰포트 캡처 `…-viewport-…`, 「턴 3」 클릭 뒤 `…-turn3-viewport-…` 추가 |
| `candidate-replay` | `…/replay` | 새 사건(AI 메시지·편집 제안·수락/거절·로컬 실행·붙여넣기 막음·포커스 비운 시간) |
| `candidate-signals` | `…/signals` | 의심 신호 2건 |
| `candidate-report` | `…/report` | 키 없음 안내 |
| `candidate-erase-confirm` | `…/conversations` | 「데이터 삭제」 확인 상자 열림(+ `…-viewport-…`) |
| `create-org-unchecked` / `create-org` | `#/`(조직 없음) | 이름 입력 후 체크 전 / 후 |
| `invite-accept` | `#/invite/<토큰>` | 초대 수락 |
| `members-terms` | `#/orgs/<org>/members` | 구성원 표 + 동의 줄 |
| `th-closeup-desktop-*` | — | 응시자 표 머리 확대 |

## 눈으로 확인한 것

- 근거 칩 클릭: 「제안 #2」·「턴 3」 모두 대상 요소(`#proposal-2`·`#turn-3`)에 `hl` 클래스가 붙고(`.hl` 개수 1, outline solid 2px — 라이트 `rgb(30,58,138)`·다크 `rgb(143,179,255)`) 뷰포트 안(위쪽 32px)으로 스크롤됐다. 데스크톱·모바일, 라이트·다크 모두 같다. 칩이 가리키는 제안 #2의 `details`는 닫힌 채라 강조된 줄은 「줄 2:1–4:1에 넣을 글 47자」 한 줄이다.
- 편집 타임라인: 제안 #1에 「수락 · 받아들인 뒤 응시자가 고침」, 코드 바뀜 칸에 「제안 #1 수락 뒤 수정」 배지가 붙고 diff 줄(+/−)의 색·정렬은 라이트·다크 모두 어긋남이 없다.
- 데이터 삭제 상자 390px: 좌우 여백 16px(상하 58px)로 가장자리에 붙지 않는다. 제목은 한 번만 나온다. 데스크톱 폭 420px.
- 구성원 화면 동의 시각: 「이용약관 2026-11-07 동의 · 2026-11-07 10:30 · 약관 보기」로 연도가 나온다. 표와 줄 사이 간격 29px. 390px에서 「약관 보기」는 한 덩어리로 다음 줄로 넘어간다.
- 초대 수락 사이드바 제목: 「조직 초대 / 초대 수락 중」.
- 조직 만들기: 입력 → 약관 체크 → 단추 순서(세로 위치 427 → 493 → 537). 체크 전 단추는 비활성(회색), 후 활성(강조색).

## 390px 가로 넘침 (`overflow.json`)

`document.documentElement.scrollWidth - clientWidth`는 **11개 화면 × 라이트·다크 모두 0px**. 안쪽 가로 스크롤 요소(접힌 아이콘 메뉴의 1px 폭 글자 칸은 제외):

| 화면 | 안쪽 스크롤 |
|---|---|
| `exam-results` | `a`(이동 경로의 시험 이름 링크 「2026 하반기 백엔드 개발자 코딩테스트」, 시험 상세로 가는 항목) 244>187 — 말줄임(…)으로 일부러 자른 것 |
| `candidate-*` 6개 화면 | 같은 이동 경로 링크 244>205(말줄임) |
| 그 밖 화면 | 없음 |

**고침 확인**: 이전에 있던 `div.sc-table-scroll` 382>356(응시자 표)은 **없어졌다**(0건). 응시자 표가 「응시자·상태·최종 점수」 세 열로 줄고 최종 점수 「67.7」·「—」가 첫 화면에 보인다. `beyond_viewport`(뷰포트 오른쪽을 넘는 요소)도 0건. `aside.sc-sidenav`·`nav.sc-tabs`도 넘치지 않는다. 남은 `a`는 의도한 말줄임이라 결함이 아니다.

## 대비 측정 (getComputedStyle, 글자색 대 겹쳐 계산한 배경색, WCAG 비)

| 요소 | 라이트 | 다크 |
|---|---|---|
| `.sc-btn--danger`(데이터 삭제) | 7.57 (흰 글자 on `#9b2c1f`) | 9.38 (어두운 글자 on 연한 살구) |
| `.sc-btn--accent`(조직 만들기 활성) | 10.36 | 9.31 |
| `.sc-badge--success` | 5.87 | 6.39 |
| `.sc-badge--danger` | 6.28 | 8.00 |
| `.sc-badge--warning` | 6.67 | 8.01 |
| `.sc-badge--info` | 8.51 | 7.09 |
| `.sc-table th` | 5.98 (`rgb(91,100,114)` on 흰색) | 7.63 |

모두 4.5 이상(수정 전과 같은 값). 390폭에서도 같다. 비활성 단추는 측정하지 않았다.
`.sc-table th`: `text-transform: uppercase`, `letter-spacing: 0.66px`. 한글에는 대문자 변환이 영향이 없고, 자간 때문에 「응시자」「최종 점수」가 약간 벌어져 보이는 정도(확대 캡처 `th-closeup-*`).

## 이번에 고쳐진 것 (이전 촬영 대비)

1. **고침** 삭제 확인 상자 안내문: 「제출 프롬프트」「공개 예시 대조」가 사라지고 「제출 코드·채점 결과」「편집 기록·대화·평가」로 바뀌었다.
2. **고침** 삭제 확인 상자 제목 중복·여백: 제목이 한 번만 나오고 좌우 안쪽 여백이 생겼으며 「닫기」 단추가 오른쪽 정렬이다(제목 밑줄 간격은 남은 결함 1).
3. **고침** 390px `exam-results` 안쪽 가로 스크롤 0건, 최종 점수 열이 보인다.
4. **고침** 390px 응시자 상세 이동 경로: 「› 2026 하반기 백엔드 개발자 코… › 응시자」로 시험 이름이 보인다.
5. **고침** 390px `members-terms`: 「약관 보기」가 「약관 보 / 기」로 깨지지 않고 한 덩어리로 줄바꿈된다.

## 남은 결함 (고치지 않음)

원인 구분: **promptlab** = `crates/dashboard`·shared 문구, **sandcorp-design** = `design/sandcorp.css`.

1. (promptlab 또는 sandcorp-design — `sc-dialog` 머리, 사소) `candidate-erase-confirm`: 제목 밑줄(빨강)이 제목 글자 바로 아래에 붙어 있고(글자와 선 사이 거의 여백 없음), 「취소」 단추는 없다(「닫기」·「영구 삭제」만, 「영구 삭제」는 이메일 입력 전 회색 비활성).
2. **고침(촬영 뒤 커밋)** 390px 이동 경로: 숨긴 칸 뒤에 첫 보이는 항목 앞 구분자 「›」가 남던 것은 promptlab의 숨김 규칙 탓이라 `style.css`에서 그 구분자를 함께 숨겼다. 이 사진들은 그 커밋 전에 찍어 「›」가 남아 있다.
3. (promptlab) `candidate-signals`: 신호 시각이 「11-05 10:06:12」로 연도가 없다(구성원 화면은 연도가 나온다).
4. (sandcorp-design) 데스크톱 전체 페이지 캡처에서 왼쪽 메뉴 배경이 뷰포트 높이(900px)에서 끊긴다.
5. (promptlab) 390px 위쪽 아이콘 메뉴가 두 줄로 접혀 일곱 번째(내 연습 기록)만 둘째 줄에 혼자 있고 이름 없는 아이콘뿐이다.
6. (promptlab) `candidate-replay`·`candidate-score`의 AI 메시지가 고정폭 `pre`로 나와 데스크톱에서 가로로 넓고 한글이 성기게 보인다(이 컨테이너 글꼴 탓일 수 있음).
7. (promptlab, 사소) 결과 머리 「최종 점수 67.7」(문항 평균)과 문항 1 점수 88.5가 함께 보여 헷갈릴 수 있다(문항 2가 낮음 — 시드 값).
8. (promptlab) `invite-accept` 다크: 체크 전 「동의하고 들어가기」가 회색 비활성이고 「나중에」가 흰 바탕으로 더 눈에 띈다.
