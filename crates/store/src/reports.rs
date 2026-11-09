//! AI 리포트 큐(#15, `exam_reports`). 응시(초대) 하나에 한 줄.
//!
//! - 자동: 모든 문항의 채점이 끝났고(완료·실패) 마지막 채점이 최근([`enqueue_ready`]의 창)인 응시를 `queued`로 넣는다.
//!   스모크 조직은 넣지 않는다. 키를 나중에 넣어도 옛 응시가 한꺼번에 몰리지 않게 창을 둔다 — 옛 응시는 요청으로.
//! - 요청: 결과 보기 역할이 [`request`]로 (다시) 만든다. 만든 횟수(실패 포함)가 상한이면 거절.
//! - 처리: 서버의 리포트 작업이 [`claim_next`]로 하나씩 꺼낸다. 조직마다 동시에 `generating`인 것이 상한을 넘지 않는다.

use promptlab_shared::report::{MAX_GENERATIONS, ReportBody, ReportStatus};
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// 저장된 리포트 한 줄.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportRow {
    pub status: ReportStatus,
    pub body: Option<ReportBody>,
    pub model: Option<String>,
    pub failure_reason: Option<String>,
    pub generations: u32,
    pub finished_at: Option<i64>,
}

fn status_of(s: &str) -> ReportStatus {
    match s {
        "queued" => ReportStatus::Queued,
        "generating" => ReportStatus::Generating,
        "done" => ReportStatus::Done,
        _ => ReportStatus::Failed,
    }
}

/// 응시 하나의 리포트.
pub async fn get(pool: &PgPool, invite_id: Uuid) -> Result<Option<ReportRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT status, body, model, failure_reason, generations,
                EXTRACT(EPOCH FROM finished_at)::bigint AS finished_at
           FROM exam_reports WHERE invite_id = $1",
    )
    .bind(invite_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let status: String = row.try_get("status")?;
        let body: Option<Json<ReportBody>> = row.try_get("body")?;
        let generations: i32 = row.try_get("generations")?;
        Ok(ReportRow {
            status: status_of(&status),
            body: body.map(|b| b.0),
            model: row.try_get("model")?,
            failure_reason: row.try_get("failure_reason")?,
            generations: u32::try_from(generations).unwrap_or(0),
            finished_at: row.try_get("finished_at")?,
        })
    })
    .transpose()
}

/// 모든 문항 채점이 끝난 지 `window_secs` 안인 응시에 리포트 자리를 만든다(스모크 조직 제외). 넣은 수.
pub async fn enqueue_ready(pool: &PgPool, now: i64, window_secs: i64) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO exam_reports (invite_id, org_id, requested_at)
         SELECT i.id, e.org_id, to_timestamp($1)
           FROM exam_invites i
           JOIN exams e ON e.id = i.exam_id
           JOIN orgs o ON o.id = e.org_id
          WHERE NOT o.is_smoke
            AND i.candidate_sub IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM exam_reports r WHERE r.invite_id = i.id)
            AND EXISTS (SELECT 1 FROM exam_items it WHERE it.exam_id = e.id)
            AND (SELECT count(*) FROM exam_items it WHERE it.exam_id = e.id)
              = (SELECT count(*) FROM submissions s
                  WHERE s.invite_id = i.id AND s.status IN ('done', 'failed'))
            AND (SELECT max(s.finished_at) FROM submissions s WHERE s.invite_id = i.id)
              > to_timestamp($1) - make_interval(secs => $2)
            -- AI 활용 평가(#99)가 끝난 뒤에(평가 결과가 리포트 입력에 든다): 평가를 기다리거나 아직 큐에 들지 않은
            -- 채점 완료 제출이 없어야 한다.
            AND NOT EXISTS (SELECT 1 FROM submissions s
                              LEFT JOIN submission_evaluations ev ON ev.submission_id = s.id
                             WHERE s.invite_id = i.id AND s.status = 'done'
                               AND (ev.status IN ('queued', 'evaluating')
                                    OR (ev.status IS NULL
                                        AND s.finished_at > to_timestamp($1) - make_interval(secs => $2))))
         ON CONFLICT (invite_id) DO NOTHING",
    )
    .bind(now as f64)
    .bind(window_secs as f64)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// [`request`]의 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    /// 큐에 넣었다.
    Queued,
    /// 이미 대기 중이거나 만드는 중이다.
    InProgress,
    /// 만든 횟수가 상한이다.
    LimitReached,
}

/// 결과 보기 역할의 (다시) 만들기 요청.
pub async fn request(
    pool: &PgPool,
    invite_id: Uuid,
    org_id: Uuid,
    now: i64,
) -> Result<RequestOutcome, sqlx::Error> {
    let inserted = sqlx::query(
        "INSERT INTO exam_reports (invite_id, org_id, requested_at) VALUES ($1, $2, to_timestamp($3))
         ON CONFLICT (invite_id) DO NOTHING",
    )
    .bind(invite_id)
    .bind(org_id)
    .bind(now as f64)
    .execute(pool)
    .await?;
    if inserted.rows_affected() == 1 {
        return Ok(RequestOutcome::Queued);
    }
    let updated = sqlx::query(
        "UPDATE exam_reports SET status = 'queued', requested_at = to_timestamp($2)
          WHERE invite_id = $1 AND status IN ('done', 'failed') AND generations < $3",
    )
    .bind(invite_id)
    .bind(now as f64)
    .bind(i32::try_from(MAX_GENERATIONS).unwrap_or(i32::MAX))
    .execute(pool)
    .await?;
    if updated.rows_affected() == 1 {
        return Ok(RequestOutcome::Queued);
    }
    Ok(match get(pool, invite_id).await?.map(|r| r.status) {
        Some(ReportStatus::Queued | ReportStatus::Generating) => RequestOutcome::InProgress,
        _ => RequestOutcome::LimitReached,
    })
}

/// 꺼낸 리포트 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    pub invite_id: Uuid,
    pub org_id: Uuid,
}

/// 가장 오래 기다린 리포트 하나를 `generating`으로 바꾸며 꺼낸다. 그 조직에서 이미 `org_limit`개가
/// 만드는 중이면 건너뛴다. 만든 횟수를 하나 올린다.
pub async fn claim_next(
    pool: &PgPool,
    org_limit: u32,
    now: i64,
) -> Result<Option<Claimed>, sqlx::Error> {
    let row = sqlx::query(
        "UPDATE exam_reports
            SET status = 'generating', started_at = to_timestamp($2), generations = generations + 1
          WHERE invite_id = (
                SELECT r.invite_id FROM exam_reports r
                 WHERE r.status = 'queued'
                   AND (SELECT count(*) FROM exam_reports g
                         WHERE g.org_id = r.org_id AND g.status = 'generating') < $1
                 ORDER BY r.requested_at, r.invite_id
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1)
        RETURNING invite_id, org_id",
    )
    .bind(i64::from(org_limit))
    .bind(now as f64)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(Claimed {
            invite_id: row.try_get("invite_id")?,
            org_id: row.try_get("org_id")?,
        })
    })
    .transpose()
}

/// 만든 리포트를 저장한다.
pub async fn finish(
    pool: &PgPool,
    invite_id: Uuid,
    body: &ReportBody,
    model: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE exam_reports
            SET status = 'done', body = $2, model = $3, failure_reason = NULL, finished_at = to_timestamp($4)
          WHERE invite_id = $1",
    )
    .bind(invite_id)
    .bind(Json(body))
    .bind(model)
    .bind(now as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// 실패로 남긴다(전에 만든 리포트 본문은 지우지 않는다 — 다시 만들기가 실패해도 앞의 것은 보인다).
pub async fn fail(
    pool: &PgPool,
    invite_id: Uuid,
    reason: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE exam_reports SET status = 'failed', failure_reason = $2, finished_at = to_timestamp($3)
          WHERE invite_id = $1",
    )
    .bind(invite_id)
    .bind(reason)
    .bind(now as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// 서버가 만드는 도중 죽어 `generating`에 남은 것을 큐로 되돌린다(서버 하나 전제). 되돌린 수.
pub async fn requeue_generating(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE exam_reports SET status = 'queued', generations = GREATEST(generations - 1, 0)
          WHERE status = 'generating'",
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// 응시(초대)의 시험.
pub async fn exam_of(pool: &PgPool, invite_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT exam_id FROM exam_invites WHERE id = $1")
        .bind(invite_id)
        .fetch_optional(pool)
        .await
}

/// 응시의 모든 문항(`item_count`개) 채점이 끝났나(완료·실패).
pub async fn grading_finished(
    pool: &PgPool,
    invite_id: Uuid,
    item_count: usize,
) -> Result<bool, sqlx::Error> {
    let finished: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM submissions WHERE invite_id = $1 AND status IN ('done', 'failed')",
    )
    .bind(invite_id)
    .fetch_one(pool)
    .await?;
    Ok(item_count > 0 && usize::try_from(finished).unwrap_or(0) >= item_count)
}
