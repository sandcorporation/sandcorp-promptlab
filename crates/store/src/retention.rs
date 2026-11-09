//! 데이터 보유·자동 삭제(#54) 저장소. 규칙은 [`promptlab_shared::retention`].
//!
//! - 보관 기간 설정·이력: [`org_days`]·[`exam_days`]·[`set_org_days`]·[`set_exam_days`]·[`history`].
//! - 기간이 지난 것 찾기: [`expired_assignments`](응시 종료 + 보관 기간 ≤ 지금)·[`inactive_practitioners`].
//! - 지우기: [`erase`] 하나 — 자동 삭제(#54)와 관리자 삭제(#55)가 같이 쓴다. 한 번에 한 트랜잭션.

use promptlab_shared::retention::{DEFAULT_DAYS, Erased, PRACTICE_INACTIVE_DAYS, RetentionChange};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

/// 배정(`i`, 시험 `e` 조인)의 응시 종료 시각(timestamptz): 마지막 활동과 응시 가능 시간 끝 중 늦은 것.
/// 링크 만료 시각은 아직 받지 않은 초대에만 센다. `GREATEST`는 NULL을 건너뛴다.
pub const ASSIGNMENT_END_SQL: &str = "GREATEST(
    i.created_at, CASE WHEN i.accepted_at IS NULL THEN i.expires_at END, i.accepted_at,
    e.window_ends_at,
    (SELECT max(st.started_at) FROM exam_item_starts st WHERE st.invite_id = i.id),
    (SELECT max(sb.created_at) FROM submissions sb WHERE sb.invite_id = i.id))";

/// 배정(`i`·시험 `e`·조직 `o` 조인)의 보관 기간(일): 시험별 → 조직 기본 → [`DEFAULT_DAYS`].
fn effective_days_sql() -> String {
    format!("COALESCE(e.retention_days, o.retention_days, {DEFAULT_DAYS})")
}

/// 지울 것.
#[derive(Debug, Clone, Copy)]
pub enum EraseTarget<'a> {
    /// 이 배정들(초대)과 거기 딸린 응시 데이터 전부. 시험 구성은 남는다.
    Assignments(&'a [Uuid]),
    /// 이 사람(`sub`)의 개인 연습 기록 전부.
    PracticeOf(&'a str),
}

fn to_days(n: Option<i32>) -> Option<u32> {
    n.and_then(|n| u32::try_from(n).ok())
}

/// 조직 기본 보관 기간(설정이 없으면 `None`).
pub async fn org_days(pool: &PgPool, org_id: Uuid) -> Result<Option<u32>, sqlx::Error> {
    let n: Option<i32> = sqlx::query_scalar("SELECT retention_days FROM orgs WHERE id = $1")
        .bind(org_id)
        .fetch_one(pool)
        .await?;
    Ok(to_days(n))
}

/// 시험별 보관 기간(설정이 없으면 `None`).
pub async fn exam_days(pool: &PgPool, exam_id: Uuid) -> Result<Option<u32>, sqlx::Error> {
    let n: Option<i32> = sqlx::query_scalar("SELECT retention_days FROM exams WHERE id = $1")
        .bind(exam_id)
        .fetch_one(pool)
        .await?;
    Ok(to_days(n))
}

async fn record_change(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    exam_id: Option<Uuid>,
    by: &str,
    now: i64,
    old: Option<i32>,
    new: Option<u32>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO retention_changes (org_id, exam_id, changed_by, changed_at, old_days, new_days)
         VALUES ($1, $2, $3, to_timestamp($4), $5, $6)",
    )
    .bind(org_id)
    .bind(exam_id)
    .bind(by)
    .bind(now as f64)
    .bind(old)
    .bind(new.map(|d| d as i32))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 조직 기본 보관 기간을 바꾸고 이력을 남긴다(값이 같으면 이력 없이 그대로). 범위 검사는 부르는 쪽이 한다.
pub async fn set_org_days(
    pool: &PgPool,
    org_id: Uuid,
    days: Option<u32>,
    by: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let old: Option<i32> =
        sqlx::query_scalar("SELECT retention_days FROM orgs WHERE id = $1 FOR UPDATE")
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await?;
    if to_days(old) != days {
        sqlx::query("UPDATE orgs SET retention_days = $2 WHERE id = $1")
            .bind(org_id)
            .bind(days.map(|d| d as i32))
            .execute(&mut *tx)
            .await?;
        record_change(&mut tx, org_id, None, by, now, old, days).await?;
    }
    tx.commit().await
}

/// 시험별 보관 기간을 바꾸고 이력을 남긴다(값이 같으면 이력 없이 그대로).
pub async fn set_exam_days(
    pool: &PgPool,
    org_id: Uuid,
    exam_id: Uuid,
    days: Option<u32>,
    by: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let old: Option<i32> = sqlx::query_scalar(
        "SELECT retention_days FROM exams WHERE id = $1 AND org_id = $2 FOR UPDATE",
    )
    .bind(exam_id)
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await?;
    if to_days(old) != days {
        sqlx::query("UPDATE exams SET retention_days = $2 WHERE id = $1")
            .bind(exam_id)
            .bind(days.map(|d| d as i32))
            .execute(&mut *tx)
            .await?;
        record_change(&mut tx, org_id, Some(exam_id), by, now, old, days).await?;
    }
    tx.commit().await
}

/// 보관 기간 변경 이력(새것부터). `exam_id`가 있으면 그 시험과 조직 기본값의 변경만.
pub async fn history(
    pool: &PgPool,
    org_id: Uuid,
    exam_id: Option<Uuid>,
) -> Result<Vec<RetentionChange>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT EXTRACT(EPOCH FROM c.changed_at)::bigint AS at, c.changed_by, u.name, c.exam_id,
                c.old_days, c.new_days
           FROM retention_changes c LEFT JOIN users u ON u.sub = c.changed_by
          WHERE c.org_id = $1 AND ($2::uuid IS NULL OR c.exam_id IS NULL OR c.exam_id = $2)
          ORDER BY c.changed_at DESC, c.id DESC",
    )
    .bind(org_id)
    .bind(exam_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(RetentionChange {
                at: r.try_get("at")?,
                by_sub: r.try_get("changed_by")?,
                by_name: r.try_get("name")?,
                exam_id: r
                    .try_get::<Option<Uuid>, _>("exam_id")?
                    .map(|id| id.to_string()),
                old_days: to_days(r.try_get("old_days")?),
                new_days: to_days(r.try_get("new_days")?),
            })
        })
        .collect()
}

/// 시험 배정마다 삭제 시각(유닉스 초). 결과 화면의 「N일 뒤 삭제」.
pub async fn assignment_delete_times(
    pool: &PgPool,
    exam_id: Uuid,
) -> Result<Vec<(Uuid, i64)>, sqlx::Error> {
    let sql = format!(
        "SELECT i.id,
                EXTRACT(EPOCH FROM {ASSIGNMENT_END_SQL})::bigint
                  + {days}::bigint * 86400 AS delete_at
           FROM exam_invites i JOIN exams e ON e.id = i.exam_id JOIN orgs o ON o.id = e.org_id
          WHERE i.exam_id = $1",
        days = effective_days_sql()
    );
    let rows = sqlx::query(&sql).bind(exam_id).fetch_all(pool).await?;
    rows.iter()
        .map(|r| Ok((r.try_get("id")?, r.try_get("delete_at")?)))
        .collect()
}

/// 삭제 시각(응시 종료 + 보관 기간)이 `now`이거나 지난 배정. 모든 조직(스모크 포함), 오래된 것부터 `limit`개.
pub async fn expired_assignments(
    pool: &PgPool,
    now: i64,
    limit: i64,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let sql = format!(
        "SELECT i.id
           FROM exam_invites i JOIN exams e ON e.id = i.exam_id JOIN orgs o ON o.id = e.org_id
          WHERE {ASSIGNMENT_END_SQL} + make_interval(days => {days}) <= to_timestamp($1)
          ORDER BY i.created_at
          LIMIT $2",
        days = effective_days_sql()
    );
    sqlx::query_scalar(&sql)
        .bind(now as f64)
        .bind(limit)
        .fetch_all(pool)
        .await
}

/// 마지막 연습 제출 뒤 [`PRACTICE_INACTIVE_DAYS`]일이 `now`이거나 지난 사람들(`sub`), `limit`개.
pub async fn inactive_practitioners(
    pool: &PgPool,
    now: i64,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT candidate_sub FROM submissions WHERE practice
          GROUP BY candidate_sub
         HAVING max(created_at) + make_interval(days => $2) <= to_timestamp($1)
          ORDER BY max(created_at)
          LIMIT $3",
    )
    .bind(now as f64)
    .bind(PRACTICE_INACTIVE_DAYS as i32)
    .bind(limit)
    .fetch_all(pool)
    .await
}

async fn exec(
    tx: &mut Transaction<'_, Postgres>,
    sql: &str,
    ids: &[Uuid],
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(sql)
        .bind(ids)
        .execute(&mut **tx)
        .await?
        .rows_affected())
}

/// **삭제 서비스 하나.** 대상의 개인 데이터를 한 트랜잭션으로 지우고 종류별 건수를 돌려준다.
/// 이미 지운 것을 다시 부르면 아무것도 하지 않고 0건(멱등).
///
/// - 배정: 비용 기록은 배정·제출 연결만 끊고(숫자·조직·시험은 남김), 응시자가 있던 배정 수를 시험의
///   `erased_attempts`에 더한 뒤, 딸린 것부터 배정까지 지운다. 웹캠 사진도 함께 지운다(#55는 즉시 삭제가 필요하다).
/// - 개인 연습: 그 사람의 연습 제출 전부. 비용 기록은 제출 연결만 끊는다.
///
/// 채점 큐에 있거나 채점 중인 제출·리포트도 그대로 지운다(#55 결정). 워커·리포트 루프는 결과를 쓸 행이
/// 사라졌으면 0행 갱신으로 건너뛰고, 비용 기록은 연결 없이 남긴다([`crate::usage::record`]).
pub async fn erase(pool: &PgPool, target: EraseTarget<'_>) -> Result<Erased, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let n = erase_in(&mut tx, target).await?;
    tx.commit().await?;
    Ok(n)
}

/// [`erase`]를 부르는 쪽의 트랜잭션 안에서 — 관리자 삭제(#55)가 배정을 잠그고 감사 기록과 함께 한 번에 커밋한다.
pub async fn erase_in(
    tx: &mut Transaction<'_, Postgres>,
    target: EraseTarget<'_>,
) -> Result<Erased, sqlx::Error> {
    let mut n = Erased::default();
    match target {
        EraseTarget::Assignments(ids) => {
            if ids.is_empty() {
                return Ok(n);
            }
            n.usage_unlinked = exec(
                tx,
                "UPDATE usage_records SET invite_id = NULL, submission_id = NULL
                  WHERE invite_id = ANY($1)
                     OR submission_id IN (SELECT id FROM submissions WHERE invite_id = ANY($1))",
                ids,
            )
            .await?;
            exec(
                tx,
                "UPDATE exams e SET erased_attempts = e.erased_attempts + c.n
                   FROM (SELECT exam_id, count(*)::int AS n FROM exam_invites
                          WHERE id = ANY($1) AND candidate_sub IS NOT NULL GROUP BY exam_id) c
                  WHERE e.id = c.exam_id",
                ids,
            )
            .await?;
            n.reports = exec(
                tx,
                "DELETE FROM exam_reports WHERE invite_id = ANY($1)",
                ids,
            )
            .await?;
            n.log_events = exec(
                tx,
                "DELETE FROM exam_log_events WHERE invite_id = ANY($1)",
                ids,
            )
            .await?;
            n.log_keys = exec(
                tx,
                "DELETE FROM exam_log_keys WHERE invite_id = ANY($1)",
                ids,
            )
            .await?;
            n.log_rejections = exec(
                tx,
                "DELETE FROM exam_log_rejections WHERE invite_id = ANY($1)",
                ids,
            )
            .await?;
            n.integrity_rejections = exec(
                tx,
                "DELETE FROM exam_integrity_rejections WHERE invite_id = ANY($1)",
                ids,
            )
            .await?;
            n.webcam_photos = exec(
                tx,
                "DELETE FROM exam_webcam_photos WHERE invite_id = ANY($1)",
                ids,
            )
            .await?;
            n.webcam_consents = exec(
                tx,
                "DELETE FROM exam_webcam_consents WHERE invite_id = ANY($1)",
                ids,
            )
            .await?;
            n.submissions =
                exec(tx, "DELETE FROM submissions WHERE invite_id = ANY($1)", ids).await?;
            n.item_starts = exec(
                tx,
                "DELETE FROM exam_item_starts WHERE invite_id = ANY($1)",
                ids,
            )
            .await?;
            n.assignments = exec(tx, "DELETE FROM exam_invites WHERE id = ANY($1)", ids).await?;
        }
        EraseTarget::PracticeOf(sub) => {
            n.usage_unlinked = sqlx::query(
                "UPDATE usage_records SET submission_id = NULL
                  WHERE submission_id IN
                        (SELECT id FROM submissions WHERE practice AND candidate_sub = $1)",
            )
            .bind(sub)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            n.submissions =
                sqlx::query("DELETE FROM submissions WHERE practice AND candidate_sub = $1")
                    .bind(sub)
                    .execute(&mut **tx)
                    .await?
                    .rows_affected();
        }
    }
    Ok(n)
}

/// 이 사람(`sub`)의 대시보드 로그인 세션을 모두 끝낸다(탈퇴). 끝낸 수.
pub async fn end_sessions_of(pool: &PgPool, sub: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM sessions WHERE sub = $1")
        .bind(sub)
        .execute(pool)
        .await?
        .rows_affected())
}
