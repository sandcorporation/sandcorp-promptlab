//! AI 활용 평가 큐(#99, `submission_evaluations`)와 최종 점수 조립. 제출 하나에 한 줄.
//!
//! - 넣기([`enqueue_ready`]): 채점이 끝난(`done`) **시험** 제출 중 채점이 [`AUTO_WINDOW_SECS`] 안에 끝난 것. 스모크 조직·
//!   개인 연습(과정 로그가 없다)은 넣지 않는다. 서버에 키가 없으면 큐를 도는 루프가 없어 「평가 꺼짐」.
//! - 꺼내기([`claim_next`]): 조직마다 동시에 `evaluating`인 것이 상한을 넘지 않게.
//! - 점수 조립([`compose`]): 저장된 채점·평가 줄을 [`promptlab_shared::evaluation::item_score`]에 넣는다.

use promptlab_shared::coding::Language;
use promptlab_shared::evaluation::{self, Evaluation, EvaluationStatus, Graded, ItemScore};
use promptlab_shared::exam::{DEFAULT_SCORE_WEIGHT, RubricItem};
use promptlab_shared::grading::GradeReport;
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// 자동 평가 창: 채점이 이 안에 끝난 제출만 큐에 넣는다(리포트 큐와 같은 3일).
pub const AUTO_WINDOW_SECS: i64 = 3 * 24 * 3600;

/// 점수를 조립할 때 서버 쪽 사정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalContext {
    /// 서버에 Claude 키가 있나(없으면 「평가 꺼짐」).
    pub enabled: bool,
    /// 서버 시계(유닉스 초).
    pub now: i64,
}

/// 점수 조립에 드는 제출 한 줄의 저장값.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreParts {
    /// `submissions.status`.
    pub status: String,
    pub report: Option<GradeReport>,
    pub finished_at: Option<i64>,
    /// 시험의 w(%) — 시험 밖 제출은 기본값.
    pub weight: u8,
    pub smoke: bool,
    /// 개인 연습 제출.
    pub practice: bool,
    /// 평가 줄(`submission_evaluations.status`·`body`).
    pub eval_status: Option<String>,
    pub eval_body: Option<Evaluation>,
}

/// 저장값과 서버 사정으로 평가 상태를 정한다.
pub fn status_of(parts: &ScoreParts, ctx: EvalContext) -> EvaluationStatus {
    if parts.status != "done" {
        return EvaluationStatus::NotEvaluated;
    }
    if parts.smoke {
        return EvaluationStatus::Smoke;
    }
    if parts.practice {
        return EvaluationStatus::Practice;
    }
    match parts.eval_status.as_deref() {
        Some("queued") => EvaluationStatus::Queued,
        Some("evaluating") => EvaluationStatus::Evaluating,
        Some("done") => EvaluationStatus::Done,
        Some("failed") => EvaluationStatus::Failed,
        _ if !ctx.enabled => EvaluationStatus::Disabled,
        // 채점이 막 끝나 아직 큐에 들어가기 전이면 곧 들어간다.
        _ if parts
            .finished_at
            .is_some_and(|at| at > ctx.now - AUTO_WINDOW_SECS) =>
        {
            EvaluationStatus::Queued
        }
        _ => EvaluationStatus::NotEvaluated,
    }
}

/// 제출 한 줄의 최종 점수·계산식.
pub fn compose(parts: ScoreParts, ctx: EvalContext) -> ItemScore {
    let status = status_of(&parts, ctx);
    let graded = match parts.status.as_str() {
        "done" => Graded::Done(parts.report.as_ref().map_or(0.0, GradeReport::pass_rate)),
        "failed" => Graded::Failed,
        _ => Graded::Pending,
    };
    evaluation::item_score(graded, parts.weight, status, parts.eval_body)
}

/// [`ScoreParts`]를 읽는 SELECT 칸(별칭 `s` = submissions, `e` = exams(LEFT), `o` = orgs(LEFT),
/// `ev` = submission_evaluations(LEFT)).
pub const SCORE_COLUMNS: &str = "s.status AS sc_status, s.report AS sc_report,
    EXTRACT(EPOCH FROM s.finished_at)::bigint AS sc_finished_at,
    e.score_weight AS sc_weight,
    COALESCE(o.is_smoke, false) AS sc_smoke, s.practice AS sc_practice, ev.status AS sc_eval_status, ev.body AS sc_eval_body";

/// [`SCORE_COLUMNS`]로 읽은 줄에서 꺼낸다. 제출이 없는 줄(LEFT JOIN)이면 `None`.
pub fn parts_from_row(row: &sqlx::postgres::PgRow) -> Result<Option<ScoreParts>, sqlx::Error> {
    let Some(status) = row.try_get::<Option<String>, _>("sc_status")? else {
        return Ok(None);
    };
    let report: Option<Json<GradeReport>> = row.try_get("sc_report")?;
    let weight: Option<i16> = row.try_get("sc_weight")?;
    let body: Option<Json<Evaluation>> = row.try_get("sc_eval_body")?;
    Ok(Some(ScoreParts {
        status,
        report: report.map(|Json(r)| r),
        finished_at: row.try_get("sc_finished_at")?,
        weight: weight
            .and_then(|w| u8::try_from(w).ok())
            .unwrap_or(DEFAULT_SCORE_WEIGHT),
        smoke: row.try_get("sc_smoke")?,
        practice: row.try_get("sc_practice")?,
        eval_status: row.try_get("sc_eval_status")?,
        eval_body: body.map(|Json(b)| b),
    }))
}

/// 제출 하나의 최종 점수. 제출이 없으면 `None`.
pub async fn score_of(
    pool: &PgPool,
    submission_id: Uuid,
    ctx: EvalContext,
) -> Result<Option<ItemScore>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT {SCORE_COLUMNS}
           FROM submissions s
           LEFT JOIN exam_invites i ON i.id = s.invite_id
           LEFT JOIN exams e ON e.id = i.exam_id
           LEFT JOIN orgs o ON o.id = s.org_id
           LEFT JOIN submission_evaluations ev ON ev.submission_id = s.id
          WHERE s.id = $1"
    ))
    .bind(submission_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some(row) => parts_from_row(&row)?.map(|p| compose(p, ctx)),
        None => None,
    })
}

/// 채점이 끝난 시험 제출을 큐에 넣는다(스모크·개인 연습 제외, 채점이 [`AUTO_WINDOW_SECS`] 안에 끝난 것만 — 키를 나중에
/// 넣어도 옛 제출이 한꺼번에 돌지 않는다). 넣은 수.
pub async fn enqueue_ready(pool: &PgPool, now: i64) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO submission_evaluations (submission_id, org_id, requested_at)
         SELECT s.id, s.org_id, to_timestamp($1)
           FROM submissions s
           JOIN orgs o ON o.id = s.org_id
          WHERE s.status = 'done' AND NOT s.practice AND s.invite_id IS NOT NULL
            AND NOT o.is_smoke
            AND s.finished_at > to_timestamp($1) - make_interval(secs => $2)
            AND NOT EXISTS (SELECT 1 FROM submission_evaluations ev WHERE ev.submission_id = s.id)
         ON CONFLICT (submission_id) DO NOTHING",
    )
    .bind(now as f64)
    .bind(AUTO_WINDOW_SECS as f64)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// 평가 한 번에 드는 저장값.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub problem_id: String,
    pub org_id: Uuid,
    pub invite_id: Uuid,
    pub position: u32,
    pub language: Language,
    /// 제출 코드(최종 코드).
    pub code: String,
    pub report: Option<GradeReport>,
    /// 시험의 추가 기준표.
    pub rubric: Vec<RubricItem>,
}

/// 평가할 제출을 읽는다. 지워졌거나 시험 제출이 아니면 `None`.
pub async fn job(pool: &PgPool, submission_id: Uuid) -> Result<Option<Job>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT s.problem_id, s.org_id, s.invite_id, s.item_position, s.language, s.code, s.report,
                e.rubric
           FROM submissions s
           JOIN exam_invites i ON i.id = s.invite_id
           JOIN exams e ON e.id = i.exam_id
          WHERE s.id = $1",
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let (Some(org_id), Some(invite_id), Some(position)) = (
        row.try_get::<Option<Uuid>, _>("org_id")?,
        row.try_get::<Option<Uuid>, _>("invite_id")?,
        row.try_get::<Option<i32>, _>("item_position")?,
    ) else {
        return Ok(None);
    };
    let language: String = row.try_get("language")?;
    let Some(language) = Language::parse(&language) else {
        return Ok(None);
    };
    let report: Option<Json<GradeReport>> = row.try_get("report")?;
    let rubric: Option<Json<Vec<RubricItem>>> = row.try_get("rubric")?;
    Ok(Some(Job {
        problem_id: row.try_get("problem_id")?,
        org_id,
        invite_id,
        position: u32::try_from(position).unwrap_or(0),
        language,
        code: row.try_get("code")?,
        report: report.map(|Json(r)| r),
        rubric: rubric.map(|Json(r)| r).unwrap_or_default(),
    }))
}

/// 시험 응시(배정)의 제출 중 평가를 기다리는 것(`queued`·`evaluating`)이 있나 — 리포트는 평가가 끝난 뒤에 만든다.
pub async fn pending_for_invite(pool: &PgPool, invite_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM submission_evaluations ev
                          JOIN submissions s ON s.id = ev.submission_id
                         WHERE s.invite_id = $1 AND ev.status IN ('queued', 'evaluating'))",
    )
    .bind(invite_id)
    .fetch_one(pool)
    .await
}

/// 제출 하나의 평가 본문(끝났을 때만).
pub async fn evaluation_of(
    pool: &PgPool,
    submission_id: Uuid,
) -> Result<Option<Evaluation>, sqlx::Error> {
    let body: Option<Option<Json<Evaluation>>> = sqlx::query_scalar(
        "SELECT body FROM submission_evaluations WHERE submission_id = $1 AND status = 'done'",
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await?;
    Ok(body.flatten().map(|Json(b)| b))
}

/// 꺼낸 평가 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    pub submission_id: Uuid,
    pub org_id: Uuid,
    /// 이번이 몇 번째 시도인가(1부터).
    pub attempt: u32,
}

/// 가장 오래 기다린 평가 하나를 `evaluating`으로 바꾸며 꺼낸다. 그 조직에서 이미
/// `org_limit`개가 평가 중이면 건너뛴다.
pub async fn claim_next(
    pool: &PgPool,
    org_limit: u32,
    now: i64,
) -> Result<Option<Claimed>, sqlx::Error> {
    let row = sqlx::query(
        "UPDATE submission_evaluations
            SET status = 'evaluating', started_at = to_timestamp($2), attempts = attempts + 1
          WHERE submission_id = (
                SELECT r.submission_id FROM submission_evaluations r
                 WHERE r.status = 'queued'
                   AND (SELECT count(*) FROM submission_evaluations g
                         WHERE g.org_id = r.org_id AND g.status = 'evaluating') < $1
                 ORDER BY r.requested_at, r.submission_id
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1)
        RETURNING submission_id, org_id, attempts",
    )
    .bind(i64::from(org_limit))
    .bind(now as f64)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let attempts: i32 = row.try_get("attempts")?;
        Ok(Claimed {
            submission_id: row.try_get("submission_id")?,
            org_id: row.try_get("org_id")?,
            attempt: u32::try_from(attempts).unwrap_or(0),
        })
    })
    .transpose()
}

/// 평가를 저장한다(지워진 제출이면 0행으로 지나간다).
pub async fn finish(
    pool: &PgPool,
    submission_id: Uuid,
    body: &Evaluation,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE submission_evaluations
            SET status = 'done', body = $2, model = $3, failure_reason = NULL, finished_at = to_timestamp($4)
          WHERE submission_id = $1",
    )
    .bind(submission_id)
    .bind(Json(body))
    .bind(&body.model)
    .bind(now as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// 실패를 남긴다. `retry`이고 시도가 [`evaluation::MAX_ATTEMPTS`]보다 적으면 큐로 되돌리고 `true`.
pub async fn fail(
    pool: &PgPool,
    submission_id: Uuid,
    reason: &str,
    retry: bool,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let status: Option<String> = sqlx::query_scalar(
        "UPDATE submission_evaluations
            SET status = CASE WHEN $3 AND attempts < $5 THEN 'queued' ELSE 'failed' END,
                failure_reason = $2, finished_at = to_timestamp($4)
          WHERE submission_id = $1
        RETURNING status",
    )
    .bind(submission_id)
    .bind(reason)
    .bind(retry)
    .bind(now as f64)
    .bind(i32::try_from(evaluation::MAX_ATTEMPTS).unwrap_or(i32::MAX))
    .fetch_optional(pool)
    .await?;
    Ok(status.as_deref() == Some("queued"))
}

/// 서버가 평가 도중 죽어 `evaluating`에 남은 것을 큐로 되돌린다(서버 하나 전제). 되돌린 수.
pub async fn requeue_evaluating(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE submission_evaluations SET status = 'queued', attempts = GREATEST(attempts - 1, 0)
          WHERE status = 'evaluating'",
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}
