//! promptlab 공유 크레이트. 서버·채점 워커·응시 앱·대시보드가 함께 쓰는 타입과 순수 로직.
//!
//! - [`api`]: HTTP API 요청·응답 타입
//! - [`account`]: 조직 역할·권한 표(서버가 강제, 대시보드가 화면에 반영)
//! - [`exam`]: 시험(문항 묶음)·응시 배정 초대의 규칙 — 용도별 결과 공개, 제한 범위, 초대 상태
//! - [`coding`]: 코딩 문제 형식(spec #93 — 숨은 테스트는 [`coding::CodingProblem`]에만, 밖으로는 [`coding::PublicCodingProblem`]) — 함수 서명·테스트 케이스·검증·결과 비교·4개 언어 틀/하네스 생성·러너 이미지 판
//! - [`authoring`]: 조직 문제 제작(#16) — 출제자의 코딩 문제 원본(히든 테스트·모범 답안 포함, 자기 조직 시험 관리 역할에게만), 저장 전 검증, 발행 전 점검
//! - [`grading`]: 채점 리포트 — 언어·통과 수만(케이스별 결과·컴파일 메시지는 밖으로 안 나간다)
//! - [`publication`]: 발행 전 점검 — 모범 답안이 예제·히든을 판정기로 모두 통과하는가
//! - [`ai`]: 응시 앱 AI 도우미(#98) — Qwen 도구 호출 템플릿·도구 목록·답 해석·줄 편집 계산·문맥 요약 규칙. 서버·store·worker는 쓰지 않는다
//! - [`log`]: 과정 로그 — 시간순 사건, 해시 체인 + Ed25519 서명, 검증, 다시보기 신호(#9)
//! - [`integrity`]: 잠금·무결성 — 앱 판·모델 해시 허용 목록, 의심 신호(#12)
//! - [`erasure`]: 고객사 관리자의 응시자 데이터 삭제·감사 로그(#55)
//! - [`report`]: AI 리포트(#15) — 리포트 모델·단가 표 한 곳, 리포트 보기·사용량 타입, 비용 계산
//! - [`inference`]: 추론 트레잇과 고정값(모델·문맥·샘플링). 응시 앱만 쓴다. llama.cpp 구현은 `promptlab-inference` 크레이트

pub mod account;
pub mod activity;
pub mod ai;
pub mod api;
pub mod authoring;
pub mod coding;
pub mod erasure;
pub mod evaluation;
pub mod exam;
pub mod grading;
pub mod inference;
pub mod integrity;
pub mod live;
pub mod log;
pub mod model_choice;
pub mod operator;
pub mod practice;
pub mod publication;
pub mod report;
pub mod retention;
pub mod syntax;
pub mod terms;
pub mod webcam;
