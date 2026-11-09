//! 개인 연습(#18): 조직·초대 없이 연습 문제를 풀고(`POST /submissions`, `lib.rs`) 내 기록을 본다.
//! 규칙은 [`promptlab_shared::practice`] 문서. 기계 주체는 이 경로들을 부를 수 없다(`auth::machine_may` → 403).

use axum::extract::State;
use axum::{Extension, Json};
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_shared::practice::{self, PracticeAttempt, PracticeRecords};
use promptlab_store::{bank, submissions};

use crate::auth::Account;
use crate::{ApiError, AppState};

/// 내 기록에 싣는 최근 제출 수.
pub const MAX_RECORDS: i64 = 500;

/// `GET /practice/problems`: 연습 문제들(공개 보기) — 내장·운영자 연습 문항 중 내리지 않은 것(#19).
pub async fn problems(
    State(state): State<AppState>,
) -> Result<Json<Vec<PublicCodingProblem>>, ApiError> {
    Ok(Json(bank::active(&state.pool, true).await?))
}

/// `GET /me/practice`: 내 연습 기록. 남의 기록은 어떤 경로로도 보이지 않는다(`sub`로만 고른다).
pub async fn records(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
) -> Result<Json<PracticeRecords>, ApiError> {
    let rows = submissions::practice_of(&state.pool, &account.sub, MAX_RECORDS).await?;
    let attempts = rows
        .into_iter()
        .map(|(view, submitted_at)| PracticeAttempt {
            submission_id: view.id,
            problem_id: view.problem_id,
            submitted_at,
            state: view.state,
        })
        .collect();
    Ok(Json(practice::summarize(attempts)))
}
