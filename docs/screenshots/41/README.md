# #41 sandcorp-design 적용 스크린샷

찍은 커밋의 로컬 스택: 서버 이미지(`docker/server.Dockerfile`, wasm 대시보드 포함) + Postgres 17, 시드는 API(세션 쿠키 `pl_session`을 `sessions`에 직접 넣음)와 SQL(제출·점수). 운영 auth·실제 API 키 없음.

## 대시보드 `dashboard/`

파일 이름 `<화면>-<desktop 1280|mobile 390>-<light|dark>.jpg`(Playwright `colorScheme`, 전체 페이지, JPEG 55).

| 화면 | 무엇 |
|---|---|
| `ai-report` | AI 리포트(키 없음 → 꺼짐 안내) |
| `erase-confirm` | 응시자 데이터 삭제 확인 |
| `exam-create` | 시험 만들기 |
| `exam-detail-results` | 채용 시험 상세(문항·응시 가능 시간·시험 보관 기간·응시자 초대·초대 현황·응시 결과·CSV) |
| `exam-internal-rounds` | 사내 진단 시험 상세(점수 공개 설정·결과) |
| `exam-invite` | 시험 초대 링크(`/exam-invite/…`, 응시자) |
| `home-org` | 머리띠·사용자 줄·내 조직·조직 화면(구성원·동료 초대·보관 기간·우리 조직 문제·시험 목록·회차 비교·사용량·삭제 기록)·내 연습 기록 |
| `live` | 실시간 보기(TURN 비밀 없음 → 503 안내) |
| `login` | 로그인 |
| `operator-allowlist` | 운영자: 허용 목록 |
| `operator-bank` | 운영자: 문제은행 |
| `operator-metrics` | 운영자: 큐·비용 |
| `operator-orgs` | 운영자: 시범 조직 승인 |
| `problem-editor` | 조직 문제 편집·미리 돌리기 |
| `replay` | 과정 다시보기 |
| `signals` | 의심 신호 |
| `webcam` | 웹캠 사진 |

### 390폭 가로 넘침(측정, `overflow.json`)

`document.documentElement.scrollWidth - clientWidth` · 뷰포트 오른쪽을 넘는 요소 수 · 안쪽 스크롤이 생긴 표·목록·입력 상자 수(라이트 / 다크). 모두 0이다.

| 화면 | 라이트 / 다크 |
|---|---|
| `ai-report` | 0px·0개·0개 / 0px·0개·0개 |
| `erase-confirm` | 0px·0개·0개 / 0px·0개·0개 |
| `exam-create` | 0px·0개·0개 / 0px·0개·0개 |
| `exam-detail-results` | 0px·0개·0개 / 0px·0개·0개 |
| `exam-internal-rounds` | 0px·0개·0개 / 0px·0개·0개 |
| `exam-invite` | 0px·0개·0개 / 0px·0개·0개 |
| `home-org` | 0px·0개·0개 / 0px·0개·0개 |
| `live` | 0px·0개·0개 / 0px·0개·0개 |
| `login` | 0px·0개·0개 / 0px·0개·0개 |
| `operator-allowlist` | 0px·0개·0개 / 0px·0개·0개 |
| `operator-bank` | 0px·0개·0개 / 0px·0개·0개 |
| `operator-metrics` | 0px·0개·0개 / 0px·0개·0개 |
| `operator-orgs` | 0px·0개·0개 / 0px·0개·0개 |
| `problem-editor` | 0px·0개·0개 / 0px·0개·0개 |
| `replay` | 0px·0개·0개 / 0px·0개·0개 |
| `signals` | 0px·0개·0개 / 0px·0개·0개 |
| `webcam` | 0px·0개·0개 / 0px·0개·0개 |

## 응시 앱 `app/`

리눅스(Xvfb, WebKitGTK, 800×600 기본 창) 위의 실제 응시 앱(`promptlab-app`, 디버그 빌드) — 가짜 auth(로컬 Ed25519 JWKS)로 받은 토큰, 로컬 서버·워커(실제 모델). 다크는 `GTK_THEME=Adwaita:dark`(웹뷰 `prefers-color-scheme: dark`). 글꼴은 컨테이너의 Noto CJK — Windows에서는 맑은 고딕으로 보인다.

| 파일 | 화면 |
|---|---|
| `start-*` | 시작(내 시험·개인 연습) |
| `assignments-*` | 내 시험 불러오기 → 배정 목록 |
| `gate-*`, `gate2-light` | 응시 전 점검·동의(웹캠 없음 상태, 수집 항목 안내) |
| `practice-list-light` | 연습 문제 목록 |
| `checking-light` | 점검 중(모델 확인) |
| `consent-light` | 문항 안내·동의 |
| `solve-light` · `example-light` | 문항 풀이(프롬프트 편집·공개 예시 시험 결과) |
| `grading-light` · `graded-light` | 제출 → 채점 중 → 채점 결과 |

전체화면 잠금 표지(「전체화면 잠금 중」)·시험 문항 풀이·「응시 완료」는 웹캠 기준 사진이 필요해 컨테이너에서는 찍지 못했다 — Windows 설치 화면 캡처로 받는다(PR 본문).
