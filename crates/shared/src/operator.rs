//! 운영자 화면(#19) API 타입. **운영자 = 대표 계정**(토큰에 `acct` 없음 + `sub` = 서버 설정의 대표 `sub`) 하나다 —
//! 아래 경로는 그 주체만 받는다(일반 사용자·기계 주체 `403`). 대시보드는 [`crate::api::MeView::is_owner`]로 메뉴를 보인다.
//!
//! | 메서드·경로 | 요청 | 응답 |
//! |---|---|---|
//! | `GET /operator/orgs` | — | `[`[`OperatorOrg`]`]`(스모크 조직 제외, 만든 순) |
//! | `PUT /operator/orgs/{id}/approval` | [`ApprovalRequest`] | [`OperatorOrg`] |
//! | `GET /operator/problems` | — | `[`[`BankProblemSummary`]`]`(내장 + 운영자 문항, 히든 테스트 없음) |
//! | `POST /operator/problems` | [`CreateBankProblemRequest`] | `201` [`BankProblem`](히든 테스트 포함) |
//! | `GET /operator/problems/{id}` | — | [`BankProblem`](운영자 문항만 — 내장은 저장소 파일이 정본이라 `404`) |
//! | `PUT /operator/problems/{id}` | [`ProblemSource`] | [`BankProblem`](운영자 문항만) |
//! | `PUT /operator/problems/{id}/retirement` | [`RetireRequest`] | [`BankProblemSummary`](내장·운영자 문항) |
//! | `POST /operator/problem-checks` | [`crate::authoring::PreviewRequest`] | `202` [`crate::authoring::PreviewView`](운영자 문항의 발행 전 점검, 진행 중 [`crate::authoring::MAX_ACTIVE_PREVIEWS`]개 — 넘으면 `429`) |
//! | `GET /operator/problem-checks/{check}` | — | [`crate::authoring::PreviewView`] |
//! | `POST /operator/problems/{id}/publish` | — | [`Publication`](운영자 문항만, 점검 미통과면 `422` + 안내) |
//! | `GET /operator/metrics` | — | [`OperatorMetrics`](큐·처리 시간·실패·조직별 비용) |
//! | `GET /operator/allowlist` | — | [`AllowlistView`] |
//! | `POST /operator/allowlist` | [`AllowlistChangeRequest`] | `201` [`AllowlistView`] |
//!
//! - **문제은행**: 내장 문항(`PROMPTLAB_PROBLEMS_DIR`의 `*.json`, #138)은 데이터 파일이 정본이다(모범 답안이
//!   묶여 있다) — 화면에서는 **내리기**만 한다(DB 표시). 운영자가 화면에서 만드는 문항은 DB(`bank_problems`, id
//!   `bank-` 접두)에 두고 모든 조직의 문제은행에 들어간다. 내린 문항은 새 시험(`422`)·연습 목록에서 빠지지만 이미
//!   만든 시험의 문항 시작·채점·과거 결과는 그대로 문제를 찾는다. 연습/시험 구분은 만들 때 정하고 바꾸지 않는다.
//!   운영자 문항도 조직 문제처럼 **발행 전 점검(#73)을 통과해 발행해야** 새 시험·연습 목록에 들어가고, 고치면 발행이
//!   풀린다(내린 것과 같은 원칙으로 이미 만든 시험은 그대로). 내장 문항은 저장소 시험이
//!   점검을 대신한다(모범 답안 판정은 `crates/judge/tests/bank.rs`).
//! - **시범 조직 승인**: 승인 전 조직은 시험을 만들 수 없고(`403`) 문항을 시작할 수 없다(`403` — 승인을 거두면
//!   이미 만든 시험도 새 문항을 못 연다). 문제 만들기·구성원 초대 같은 준비는 승인 전에도 된다.
//! - **허용 목록**: [`crate::integrity::Allowlist`] — 상수 ∪ 더한 값 − 막은 값, 변경은 이력으로 남는다.
//!
//! 히든 테스트이 실리는 응답은 [`BankProblem`](운영자 문항 편집)뿐이다.

use serde::{Deserialize, Serialize};

use crate::authoring::{ProblemSource, Publication};
use crate::integrity::{AllowAction, AllowKind, Allowlist};
use crate::report::KindUsage;

/// 운영자 문항 id 접두.
pub const BANK_PROBLEM_PREFIX: &str = "bank-";

/// 지표의 「최근」 창(시간).
pub const METRICS_WINDOW_HOURS: i64 = 24;

/// 최근 실패 목록 최대.
pub const MAX_RECENT_FAILURES: i64 = 20;

/// 조직 하나(운영자 보기).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorOrg {
    pub id: String,
    pub name: String,
    pub created_by: String,
    /// 유닉스 초.
    pub created_at: i64,
    /// 승인 시각(유닉스 초). 없으면 승인 대기.
    pub approved_at: Option<i64>,
    pub members: i64,
    pub exams: i64,
}

/// `PUT /operator/orgs/{id}/approval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// `true` 승인, `false` 승인 거두기.
    pub approved: bool,
}

/// 문항의 출처.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemOrigin {
    /// 저장소 파일(내리기만 된다).
    Builtin,
    /// 운영자가 화면에서 만든 문항.
    Operator,
}

/// 문제은행 문항 한 줄(히든 테스트 없음).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BankProblemSummary {
    pub id: String,
    pub origin: ProblemOrigin,
    /// 연습 문항이면 `true`(연습에만), 아니면 시험 문항.
    pub practice: bool,
    pub title: String,
    pub example_count: usize,
    pub hidden_count: usize,
    /// 운영자 문항을 마지막으로 고친 시각(내장은 없음).
    pub updated_at: Option<i64>,
    /// 내린 시각. 없으면 쓰는 중.
    pub retired_at: Option<i64>,
    pub retired_reason: Option<String>,
    /// 발행했다(내장은 늘 발행 — 저장소 시험이 점검한다). 안 했으면 새 시험·연습 목록에 안 나온다.
    pub published: bool,
}

/// 운영자 문항(편집용, 히든 테스트 포함).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BankProblem {
    pub id: String,
    pub practice: bool,
    pub source: ProblemSource,
    pub updated_at: i64,
    pub retired_at: Option<i64>,
    /// 발행 상태와 지금 원본의 마지막 점검(#73).
    pub publication: Publication,
}

/// `POST /operator/problems`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateBankProblemRequest {
    pub practice: bool,
    pub source: ProblemSource,
}

/// `PUT /operator/problems/{id}/retirement`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetireRequest {
    /// `true` 내리기, `false` 되살리기.
    pub retired: bool,
    /// 내리는 까닭(유출 등). 되살릴 때는 무시.
    #[serde(default)]
    pub reason: String,
}

/// 내리는 까닭 최대 길이.
pub const MAX_RETIRE_REASON_CHARS: usize = 200;

/// 큐 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueKind {
    /// 시험·조직 제출 채점.
    Grading,
    /// 개인 연습 채점(#18).
    PracticeGrading,
    /// 발행 전 점검(#73 — #16 미리 돌리기 큐를 다시 쓴다, 조직 문제·운영자 문항).
    Preview,
    /// AI 리포트(#15).
    Report,
}

impl QueueKind {
    pub fn label(self) -> &'static str {
        match self {
            QueueKind::Grading => "채점(시험)",
            QueueKind::PracticeGrading => "채점(개인 연습)",
            QueueKind::Preview => "발행 전 점검",
            QueueKind::Report => "AI 리포트",
        }
    }
}

/// 큐 하나의 지금 길이와 최근 [`METRICS_WINDOW_HOURS`]시간 처리. 스모크 조직은 뺀다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueStats {
    pub kind: QueueKind,
    /// 기다리는 일.
    pub waiting: i64,
    /// 처리 중인 일.
    pub running: i64,
    /// 가장 오래 기다린 일의 대기 초.
    pub oldest_waiting_secs: Option<i64>,
    /// 최근 끝난(성공) 일.
    pub done: i64,
    /// 최근 실패한 일.
    pub failed: i64,
    /// 최근 끝난 일의 평균 대기 초(넣음 → 시작, 시작 시각이 없는 큐는 넣음 → 끝).
    pub avg_wait_secs: Option<f64>,
    /// 최근 끝난 일의 평균 처리 초(시작 → 끝). 시작 시각이 없는 큐는 없음.
    pub avg_run_secs: Option<f64>,
}

/// 최근 실패 하나. 이유는 응시자·출제자에게 보이는 짧은 이유다(채점 상세 `failure_detail`은 싣지 않는다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFailure {
    pub kind: QueueKind,
    /// 조직 이름(개인 연습은 없음).
    pub org: Option<String>,
    pub reason: String,
    /// 유닉스 초.
    pub at: i64,
}

/// 조직 하나(또는 개인 연습 묶음)의 토큰·추정 비용(#15 집계).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgCost {
    /// 조직 id. 개인 연습 묶음이면 없음.
    pub org_id: Option<String>,
    /// 조직 이름. 개인 연습 묶음이면 「개인 연습」.
    pub name: String,
    /// 응시 수(조직 = 배정받은 초대, 개인 연습 = 연습 제출).
    pub attempts: u64,
    pub by_kind: Vec<KindUsage>,
    pub cost_usd: f64,
}

/// `GET /operator/metrics`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperatorMetrics {
    pub window_hours: i64,
    pub queues: Vec<QueueStats>,
    pub failures: Vec<RecentFailure>,
    /// 조직별(비용 큰 순), 마지막 줄은 개인 연습(기록이 있을 때).
    pub costs: Vec<OrgCost>,
    pub total_cost_usd: f64,
}

/// 허용 목록 변경 하나(이력).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowlistChange {
    pub kind: AllowKind,
    pub value: String,
    pub action: AllowAction,
    pub note: String,
    pub changed_by: String,
    /// 유닉스 초.
    pub changed_at: i64,
}

/// `GET /operator/allowlist` — 상수 기본값, 지금 받는 목록, 변경 이력(새것부터).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowlistView {
    pub constants: Allowlist,
    pub effective: Allowlist,
    pub history: Vec<AllowlistChange>,
}

/// `POST /operator/allowlist`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowlistChangeRequest {
    pub kind: AllowKind,
    pub value: String,
    pub action: AllowAction,
    #[serde(default)]
    pub note: String,
}

/// 변경 메모 최대 길이.
pub const MAX_ALLOW_NOTE_CHARS: usize = 200;
