//! 응시 앱의 UI 밖 로직. 화면(`main.rs`)은 얇게 두고 시험은 여기를 친다.
//!
//! - [`session`]: 응시 세션 상태 기계(시험 이음매 ③)
//! - [`auth`]: sandcorp-auth 로그인(공개 클라이언트, PKCE, 루프백 리다이렉트)
//! - [`client`]: 서버 API 클라이언트와 설정(환경변수)
//! - [`local`]: 로컬 모델(Qwen2.5-Coder 7B, 별도 스레드에서 추론)
//! - [`assistant`]: AI 도우미(#98) — 문제마다 대화 하나, 도구 실행 루프(`read_problem`·`read_code`·`propose_edit`), 편집 제안 수락·거절, 문맥 요약
//! - [`lockdown`]: 응시 모드 잠금 — 전체화면, 포커스·전체화면 이탈·붙여넣기를 과정 로그 사건으로(#12)
//! - [`proclog`]: 과정 로그 — 설치 키 보관, 사건 서명·디스크에 쌓기, 재연결 뒤 순서대로 올리기(#9)
//! - [`workspace`]: 응시 화면의 배치 기준폭·단축키·결과 문구(#74·#97)
//! - [`editor`]: 언어별 코드 초안·코드 스냅숏 주기·클립보드(앱 안 버퍼) 판정(#97)
//! - [`editor_assets`]: 앱에 구운 Monaco 편집기 자산(#97)
//! - [`localrun`]: 응시자 PC Docker로 「실행」·「예제 테스트」, docker 오류의 사람 말(#97)
//! - [`docker_check`]: 응시 전 Docker 점검 상태 기계 — 데몬·이미지 받기·빈 하네스(#97)
//! - [`exam`]: 부수 효과를 돌려 세션 사건으로 바꾸는 응시 흐름 — 화면과 CLI가 같이 쓴다
//!
//! 앱은 공개 문제([`promptlab_shared::coding::PublicCodingProblem`])와 채점 리포트만 받는다.
//! 숨은 입력이 든 문제 목록(`promptlab-store`)에는 기대지 않는다.

pub mod assistant;
pub mod auth;
pub mod client;
pub mod docker_check;
pub mod editor;
pub mod editor_assets;
pub mod exam;
pub mod live;
pub mod local;
pub mod localrun;
pub mod lockdown;
pub mod model_pick;
pub mod proclog;
pub mod session;
pub mod webcam;
pub mod webview_data;
pub mod workspace;
