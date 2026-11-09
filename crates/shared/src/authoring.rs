//! 조직 문제 제작(#16): 출제자가 쓰는 문제 원본, 저장 전 검증, 발행 전 점검(#73)·발행의 API 타입.
//!
//! **[`ProblemSource`]·[`OrgProblem`]에는 히든 테스트(인자·기대값)·모범 답안이 일부러 들어 있다.** 자기 조직의 시험 관리 역할
//! (관리자·출제자)에게만 응답으로 나간다 — 서버가 [`crate::account::Permission::ManageExams`]로 막는다.
//! 응시자·앱·다른 역할·다른 조직에 가는 응답에는 [`CodingProblem::public_view`]의 [`crate::coding::PublicCodingProblem`]만 싣는다.
//! 서버(`crates/server/src/problems.rs`)와 MCP 출제 서버(#17)가 같은 검증([`ProblemSource::validate`])을 쓴다.
//!
//! | 메서드·경로 | 누가 | 요청 | 응답 |
//! |---|---|---|---|
//! | `GET /orgs/{id}/problems` | 시험 관리 | — | `[`[`OrgProblemSummary`]`]`(히든 테스트 없음) |
//! | `POST /orgs/{id}/problems` | 시험 관리 | [`ProblemSource`] | `201` [`OrgProblem`] |
//! | `GET /orgs/{id}/problems/{problem}` | 시험 관리 | — | [`OrgProblem`](히든 테스트·모범 답안 포함) |
//! | `PUT /orgs/{id}/problems/{problem}` | 시험 관리 | [`ProblemSource`] | [`OrgProblem`] |
//! | `POST /orgs/{id}/problem-previews` | 시험 관리 | [`PreviewRequest`] | `202` [`PreviewView`](저장된 문제의 발행 전 점검을 큐에 넣음, 조직마다 진행 중 [`MAX_ACTIVE_PREVIEWS`]개 — 넘으면 `429`) |
//! | `GET /orgs/{id}/problem-previews/{preview}` | 시험 관리 | — | [`PreviewView`](2초마다 폴링) |
//! | `POST /orgs/{id}/problems/{problem}/publish` | 시험 관리 | — | [`Publication`](지금 원본의 마지막 점검이 모두 통과일 때만, 아니면 `422` + 안내) |
//!
//! 다른 조직의 문제·점검은 `404`, 역할이 모자라면 `403`. 검증에 걸리면 `422`(까닭은 [`ProblemSource::validate`]).
//! 점검·발행 규칙은 [`crate::publication`]. 점검 결과([`PreviewView`])에는 케이스 입력·출력이 없다(통과 수·짧은 이유만).
//! 운영자 문항(#19)도 같은 타입·규칙을 쓴다(경로는 [`crate::operator`]).

use serde::{Deserialize, Serialize};

use crate::coding::{CodingProblem, Language, Limits, Signature, TestCase};
use crate::publication::CheckResult;

/// 조직 문제 id 접두(내장 문제은행 id와 겹치지 않는다).
pub const ORG_PROBLEM_PREFIX: &str = "org-";
/// 제목 최대 글자 수.
pub const MAX_TITLE_CHARS: usize = 100;
/// 설명(마크다운) 최대 글자 수.
pub const MAX_DESCRIPTION_CHARS: usize = 8000;
/// 모범 답안 최대 바이트([`crate::coding::MAX_CODE_BYTES`]와 같다).
pub const MAX_REFERENCE_BYTES: usize = crate::coding::MAX_CODE_BYTES;
/// 조직마다(운영자 문항은 운영자 전체) 동시에 진행 중(대기·실행)인 발행 전 점검 수.
pub const MAX_ACTIVE_PREVIEWS: i64 = 3;

/// 출제자가 쓰는 코딩 문제 원본(편집 API·MCP 공용). 저장하면 id가 붙어 [`CodingProblem`]이 된다.
/// DB(`org_problems`·`bank_problems`·`problem_previews`)의 원본 jsonb도 이 꼴이다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProblemSource {
    pub title: String,
    /// 마크다운 설명.
    pub description: String,
    pub signature: Signature,
    /// 공개 예제 케이스(응시자에게 나간다).
    pub examples: Vec<TestCase>,
    /// 히든 케이스(응시자에게 나가지 않는다 — 통과 수만).
    pub hidden: Vec<TestCase>,
    /// 모범 답안(발행 전 점검이 판정기로 돌린다). 응시자에게 나가지 않는다.
    pub reference_solution: String,
    #[serde(default)]
    pub reference_language: Language,
    #[serde(default)]
    pub limits: Limits,
}

impl ProblemSource {
    /// 채점·점검에 쓰는 문제.
    pub fn to_problem(&self, id: &str) -> CodingProblem {
        CodingProblem {
            id: id.to_owned(),
            title: self.title.trim().to_owned(),
            description: self.description.trim().to_owned(),
            signature: self.signature.clone(),
            examples: self.examples.clone(),
            hidden: self.hidden.clone(),
            reference_solution: self.reference_solution.clone(),
            reference_language: self.reference_language,
            limits: self.limits,
        }
    }

    /// 저장·점검 전 검증: 제목·설명·모범 답안 길이 + [`CodingProblem::validate`](서명·케이스 개수·값 타입·크기·제한).
    /// 잘못된 곳을 사람이 읽을 말로 돌려준다(서버 `422`의 본문).
    pub fn validate(&self) -> Result<(), String> {
        let title = self.title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
            return Err(format!("제목은 1~{MAX_TITLE_CHARS}자다"));
        }
        let description = self.description.trim();
        if description.is_empty() || description.chars().count() > MAX_DESCRIPTION_CHARS {
            return Err(format!("설명은 1~{MAX_DESCRIPTION_CHARS}자다"));
        }
        if self.reference_solution.len() > MAX_REFERENCE_BYTES {
            return Err(format!("모범 답안은 {MAX_REFERENCE_BYTES}바이트까지다"));
        }
        self.to_problem("").validate().map_err(|e| e.to_string())
    }
}

/// 저장된 조직 문제(편집 화면용, 히든 테스트·모범 답안 포함).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OrgProblem {
    pub id: String,
    pub source: ProblemSource,
    /// 마지막으로 고친 시각(유닉스 초).
    pub updated_at: i64,
    /// 발행 상태와 지금 원본의 마지막 점검.
    pub publication: Publication,
}

/// 조직 문제 목록의 한 줄(히든 테스트 없음).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OrgProblemSummary {
    pub id: String,
    pub title: String,
    pub example_count: usize,
    pub hidden_count: usize,
    pub updated_at: i64,
    /// 발행했는가(고치면 다시 점검·발행해야 한다).
    pub published: bool,
}

/// 발행 전 점검 요청: 저장된 문제 하나(지금 원본으로 점검한다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PreviewRequest {
    pub problem_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum PreviewState {
    /// 큐에서 기다린다(채점이 먼저다).
    Queued,
    /// 모범 답안을 판정기로 돌리는 중.
    Running,
    /// 다 돌렸다.
    Done,
    /// 돌리지 못했다(`failure_reason`).
    Failed,
}

/// 발행 전 점검의 상태·결과(출제자에게만, 케이스 입력·출력 없음).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PreviewView {
    pub id: String,
    pub problem_id: String,
    pub state: PreviewState,
    /// 요청 시각(유닉스 초).
    pub created_at: i64,
    /// 이 점검이 문제의 지금 원본에 대한 것인가 — 고친 뒤면 `false`(다시 점검해야 발행).
    pub current: bool,
    pub example_count: usize,
    pub hidden_count: usize,
    /// 모범 답안 판정 결과(돌린 뒤에만).
    pub result: Option<CheckResult>,
    /// 다 돌렸고 모범 답안이 모두 통과했다(지금 원본이면 발행할 수 있다).
    pub publishable: bool,
    /// 발행할 수 없는 짧은 이유(다 돌렸을 때만, [`CheckResult::reason`]).
    pub reason: Option<String>,
    /// 앞에 기다리는 일(채점 대기 제출 + 앞선 점검) 수. 대기 중일 때만.
    pub queue_ahead: Option<i64>,
    pub failure_reason: Option<String>,
}

impl PreviewView {
    /// 결과로 발행 가능·이유를 채운다(다 돌렸을 때).
    pub fn judge(&mut self) {
        match (&self.state, &self.result) {
            (PreviewState::Done, Some(result)) => {
                self.publishable = result.all_passed();
                self.reason = result.reason();
            }
            _ => {
                self.publishable = false;
                self.reason = None;
            }
        }
    }
}

/// 문제의 발행 상태.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Publication {
    /// 발행했다 — 시험 만들기(연습 문항이면 연습 목록)에 쓸 수 있다. 고치면 `false`로 돌아간다.
    pub published: bool,
    /// 발행 시각(유닉스 초).
    pub published_at: Option<i64>,
    /// 지금 원본에 대한 마지막 점검(없으면 점검 안 함 또는 고친 뒤 재검사 전).
    pub last_check: Option<PreviewView>,
}
