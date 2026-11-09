//! 제출 저장과 채점 큐. 큐는 `submissions` 테이블 자체다.
//!
//! 상태: `queued` → (워커가 [`claim_next`]) → `grading` → [`complete`] `done` / [`fail`] `failed`.
//! 워커가 채점 도중 죽으면 행이 `grading`에 남는다 — 워커가 시작할 때 [`requeue_grading`]으로
//! 되돌린다(채점 워커는 하나만 돈다는 전제, #5). 판정기가 잠시 안 되면(503·연결 실패) [`requeue`]로 되돌리고
//! [`MAX_ATTEMPTS`]번 꺼낸 뒤에는 실패로 남긴다.

use promptlab_shared::api::{SubmissionState, SubmissionView};
use promptlab_shared::coding::Language;
use promptlab_shared::grading::GradeReport;
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// 판정기 일시 오류로 되돌리는 최대 횟수 — 이만큼 꺼낸 뒤에도 안 되면 채점 실패.
pub const MAX_ATTEMPTS: u32 = 3;

/// 새로 낼 제출: 언어와 코드.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSubmission {
    pub language: Language,
    pub code: String,
    pub elapsed_secs: u32,
}

/// 워커가 큐에서 꺼낸 제출.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    pub id: Uuid,
    pub problem_id: String,
    pub language: Language,
    pub code: String,
    pub elapsed_secs: u32,
    /// 이번까지 꺼낸 횟수(1부터).
    pub attempts: u32,
    /// 시험 응시면 그 문항. 개인 연습은 `None`.
    pub item: Option<ItemRef>,
}

fn to_i32(n: u32) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn to_u32(n: i32) -> u32 {
    u32::try_from(n).unwrap_or(0)
}

/// 누가(어느 조직으로) 낸 제출인가. 볼 수 있는 사람을 정한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// 낸 사람(sandcorp-auth `sub`).
    pub candidate_sub: String,
    /// 조직 응시면 그 조직. 개인 제출이면 `None`.
    pub org_id: Option<Uuid>,
    /// 시험 응시(배정의 문항)면 그 문항. 문항마다 제출은 하나다.
    pub item: Option<ItemRef>,
}

/// 시험 배정의 문항 하나.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemRef {
    pub invite_id: Uuid,
    pub position: u32,
}

/// 같은 문항을 두 번 내려 할 때처럼 고유 제약에 걸린 오류인가.
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|code| code == "23505")
}

impl Owner {
    /// 조직·시험 배정 없이 낸 개인 연습인가(#18).
    pub fn is_practice(&self) -> bool {
        self.org_id.is_none() && self.item.is_none()
    }
}

/// 제출을 저장하고 큐에 넣는다. 조직·배정이 없으면 개인 연습으로 표시한다(`practice`).
pub async fn insert(
    pool: &PgPool,
    owner: &Owner,
    problem_id: &str,
    submission: &NewSubmission,
) -> Result<SubmissionView, sqlx::Error> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO submissions
            (problem_id, language, code, elapsed_secs,
             candidate_sub, org_id, invite_id, item_position, practice)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(problem_id)
    .bind(submission.language.as_str())
    .bind(&submission.code)
    .bind(to_i32(submission.elapsed_secs))
    .bind(&owner.candidate_sub)
    .bind(owner.org_id)
    .bind(owner.item.map(|i| i.invite_id))
    .bind(owner.item.map(|i| to_i32(i.position)))
    .bind(owner.is_practice())
    .fetch_one(pool)
    .await?;
    Ok(SubmissionView {
        id: id.to_string(),
        problem_id: problem_id.to_owned(),
        org_id: owner.org_id.map(|o| o.to_string()),
        state: SubmissionState::Queued,
        score: None,
    })
}

const VIEW_COLUMNS: &str = "id, problem_id, org_id, candidate_sub, invite_id, item_position, status, report, failure_reason";

/// 제출 하나의 지금 상태와 주인. 없으면 `None`.
pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<(SubmissionView, Owner)>, sqlx::Error> {
    sqlx::query(&format!(
        "SELECT {VIEW_COLUMNS} FROM submissions WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(|row| decode_view(&row))
    .transpose()
}

/// 조직의 제출들(새것부터, 최대 `limit`개).
pub async fn list_for_org(
    pool: &PgPool,
    org_id: Uuid,
    limit: i64,
) -> Result<Vec<SubmissionView>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {VIEW_COLUMNS} FROM submissions WHERE org_id = $1
          ORDER BY created_at DESC, id LIMIT $2"
    ))
    .bind(org_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| decode_view(row).map(|(view, _)| view))
        .collect()
}

/// 이 사람의 채점을 기다리는(대기·채점 중) 연습 제출 수.
pub async fn pending_practice(pool: &PgPool, candidate_sub: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM submissions
          WHERE practice AND candidate_sub = $1 AND status IN ('queued', 'grading')",
    )
    .bind(candidate_sub)
    .fetch_one(pool)
    .await
}

/// 이 사람의 연습 제출들(새것부터, 최대 `limit`개)과 낸 시각(유닉스 초).
pub async fn practice_of(
    pool: &PgPool,
    candidate_sub: &str,
    limit: i64,
) -> Result<Vec<(SubmissionView, i64)>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {VIEW_COLUMNS}, extract(epoch FROM created_at)::bigint AS submitted_at
           FROM submissions WHERE practice AND candidate_sub = $1
          ORDER BY created_at DESC, id LIMIT $2"
    ))
    .bind(candidate_sub)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| Ok((decode_view(row)?.0, row.try_get("submitted_at")?)))
        .collect()
}

fn decode_view(row: &sqlx::postgres::PgRow) -> Result<(SubmissionView, Owner), sqlx::Error> {
    let id: Uuid = row.try_get("id")?;
    let org_id: Option<Uuid> = row.try_get("org_id")?;
    let status: String = row.try_get("status")?;
    let state = match status.as_str() {
        "queued" => SubmissionState::Queued,
        "grading" => SubmissionState::Grading,
        "done" => {
            let Json(report): Json<GradeReport> = row.try_get("report")?;
            SubmissionState::Done { report }
        }
        "failed" => SubmissionState::Failed {
            reason: row.try_get("failure_reason")?,
        },
        other => {
            return Err(sqlx::Error::Decode(
                format!("알 수 없는 제출 상태 {other}").into(),
            ));
        }
    };
    Ok((
        SubmissionView {
            id: id.to_string(),
            problem_id: row.try_get("problem_id")?,
            org_id: org_id.map(|o| o.to_string()),
            state,
            score: None,
        },
        Owner {
            candidate_sub: row.try_get("candidate_sub")?,
            org_id,
            item: match (
                row.try_get::<Option<Uuid>, _>("invite_id")?,
                row.try_get::<Option<i32>, _>("item_position")?,
            ) {
                (Some(invite_id), Some(position)) => Some(ItemRef {
                    invite_id,
                    position: to_u32(position),
                }),
                _ => None,
            },
        },
    ))
}

/// 큐에서 가장 오래 기다린 제출 하나를 `grading`으로 바꾸며 꺼낸다. 비었으면 `None`.
/// 개인 연습(#18)은 시험·조직 제출이 하나도 기다리지 않을 때만 꺼낸다 — 연습이 시험 채점을 굶기지 않는다.
/// 여러 워커가 동시에 불러도 같은 행을 두 번 꺼내지 않는다(`FOR UPDATE SKIP LOCKED`).
pub async fn claim_next(pool: &PgPool) -> Result<Option<Claimed>, sqlx::Error> {
    let row = sqlx::query(
        "UPDATE submissions
            SET status = 'grading', started_at = now(), attempts = attempts + 1
          WHERE id = (
                SELECT id FROM submissions
                 WHERE status = 'queued'
                 ORDER BY practice, created_at, id
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1)
        RETURNING id, problem_id, language, code, elapsed_secs, attempts, invite_id, item_position",
    )
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let language: String = row.try_get("language")?;
        let language = Language::parse(&language)
            .ok_or_else(|| sqlx::Error::Decode(format!("알 수 없는 언어 {language}").into()))?;
        let item = match (
            row.try_get::<Option<Uuid>, _>("invite_id")?,
            row.try_get::<Option<i32>, _>("item_position")?,
        ) {
            (Some(invite_id), Some(position)) => Some(ItemRef {
                invite_id,
                position: to_u32(position),
            }),
            _ => None,
        };
        Ok(Claimed {
            id: row.try_get("id")?,
            problem_id: row.try_get("problem_id")?,
            language,
            code: row.try_get("code")?,
            elapsed_secs: to_u32(row.try_get("elapsed_secs")?),
            attempts: to_u32(row.try_get("attempts")?),
            item,
        })
    })
    .transpose()
}

/// 채점을 마치고 리포트를 저장한다.
pub async fn complete(pool: &PgPool, id: Uuid, report: &GradeReport) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE submissions
            SET status = 'done', report = $2, finished_at = now()
          WHERE id = $1 AND status = 'grading'",
    )
    .bind(id)
    .bind(Json(report))
    .execute(pool)
    .await?;
    Ok(())
}

/// 채점하지 못했다고 남긴다. `reason`은 응시자에게 보이고, `detail`은 서버 안(DB)에만 남는다.
pub async fn fail(pool: &PgPool, id: Uuid, reason: &str, detail: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE submissions
            SET status = 'failed', failure_reason = $2, failure_detail = $3, finished_at = now()
          WHERE id = $1 AND status = 'grading'",
    )
    .bind(id)
    .bind(reason)
    .bind(detail)
    .execute(pool)
    .await?;
    Ok(())
}

/// `grading`에 남은 제출(채점 도중 워커가 죽었다)을 큐로 되돌린다. 되돌린 수를 준다.
pub async fn requeue_grading(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE submissions SET status = 'queued', started_at = NULL WHERE status = 'grading'",
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// 판정기 일시 오류로 채점하지 못한 제출 하나를 큐로 되돌린다(`attempts`는 그대로 — 다음 꺼냄에서 는다).
pub async fn requeue(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE submissions SET status = 'queued', started_at = NULL
          WHERE id = $1 AND status = 'grading'",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 제출 코드(채점 워커·감독자 화면용 — 응시자 본인 외 응시 앱에는 나가지 않는다). 없으면 `None`.
pub async fn code_of(pool: &PgPool, id: Uuid) -> Result<Option<(Language, String)>, sqlx::Error> {
    let row = sqlx::query("SELECT language, code FROM submissions WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let language: String = row.try_get("language")?;
    Ok(Language::parse(&language).map(|l| (l, row.try_get("code").unwrap_or_default())))
}
