//! 고객사 관리자의 응시자 데이터 삭제와 감사 로그(#55). 규칙은 [`promptlab_shared::erasure`].
//!
//! 지우기는 #54의 삭제 서비스([`crate::retention::erase_in`]) 그대로다. 여기서는 배정을 잠그고(`FOR UPDATE`) 지운 뒤
//! 감사 기록을 같은 트랜잭션으로 남긴다 — 같은 배정을 두 사람이 동시에 지워도 하나는 잠금을 기다렸다가 행이 없음을 보고
//! 먼저 남은 감사 기록을 돌려받는다(멱등).

use promptlab_shared::erasure::{AUDIT_RETENTION_DAYS, ErasureRecord, subject_hash};
use promptlab_shared::retention::Erased;
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::retention::{EraseTarget, erase_in};

/// [`erase_assignment`]의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EraseOutcome {
    /// 이번에 지웠다.
    Erased(ErasureRecord),
    /// 이미 지운 배정이다 — 그때의 감사 기록.
    Already(ErasureRecord),
    /// 그 시험에 그런 배정이 없고 지운 기록도 없다.
    NotFound,
}

/// 지우는 사람·사유·시각.
#[derive(Debug, Clone, Copy)]
pub struct EraseBy<'a> {
    pub sub: &'a str,
    pub reason: Option<&'a str>,
    pub now: i64,
}

/// 조직 `org_id`의 시험 `exam_id` 안 배정 `invite_id`를 지우고 감사 기록을 남긴다.
pub async fn erase_assignment(
    pool: &PgPool,
    org_id: Uuid,
    exam_id: Uuid,
    invite_id: Uuid,
    by: EraseBy<'_>,
) -> Result<EraseOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let email: Option<String> = sqlx::query_scalar(
        "SELECT i.email FROM exam_invites i JOIN exams e ON e.id = i.exam_id
          WHERE i.id = $1 AND i.exam_id = $2 AND e.org_id = $3
            FOR UPDATE OF i",
    )
    .bind(invite_id)
    .bind(exam_id)
    .bind(org_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(email) = email else {
        tx.rollback().await?;
        return Ok(match find(pool, org_id, exam_id, invite_id).await? {
            Some(record) => EraseOutcome::Already(record),
            None => EraseOutcome::NotFound,
        });
    };
    let erased = erase_in(&mut tx, EraseTarget::Assignments(&[invite_id])).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO erasure_audit
             (org_id, exam_id, invite_id, subject_hash, erased_by, erased_at, reason, counts)
         VALUES ($1, $2, $3, $4, $5, to_timestamp($6), $7, $8)
         RETURNING id",
    )
    .bind(org_id)
    .bind(exam_id)
    .bind(invite_id)
    .bind(subject_hash(&org_id.to_string(), &email))
    .bind(by.sub)
    .bind(by.now as f64)
    .bind(by.reason)
    .bind(Json(erased))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let record = select(pool, org_id, Filter::Id(id))
        .await?
        .into_iter()
        .next()
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(EraseOutcome::Erased(record))
}

async fn find(
    pool: &PgPool,
    org_id: Uuid,
    exam_id: Uuid,
    invite_id: Uuid,
) -> Result<Option<ErasureRecord>, sqlx::Error> {
    Ok(select(pool, org_id, Filter::Invite(exam_id, invite_id))
        .await?
        .into_iter()
        .next())
}

/// 조직의 감사 기록(새것부터). `subject`가 있으면 그 응시자 식별자 해시([`subject_hash`])만.
pub async fn list(
    pool: &PgPool,
    org_id: Uuid,
    subject: Option<&str>,
) -> Result<Vec<ErasureRecord>, sqlx::Error> {
    select(pool, org_id, Filter::Subject(subject)).await
}

/// 보관 기간([`AUDIT_RETENTION_DAYS`])이 `now`이거나 지난 감사 기록을 지운다. 지운 수.
pub async fn purge(pool: &PgPool, now: i64) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "DELETE FROM erasure_audit WHERE erased_at + make_interval(days => $2) <= to_timestamp($1)",
    )
    .bind(now as f64)
    .bind(AUDIT_RETENTION_DAYS as i32)
    .execute(pool)
    .await?
    .rows_affected())
}

enum Filter<'a> {
    Id(i64),
    /// (시험, 배정)
    Invite(Uuid, Uuid),
    Subject(Option<&'a str>),
}

async fn select(
    pool: &PgPool,
    org_id: Uuid,
    filter: Filter<'_>,
) -> Result<Vec<ErasureRecord>, sqlx::Error> {
    let cond = match filter {
        Filter::Id(_) => "a.id = $2",
        Filter::Invite(..) => "a.exam_id = $2 AND a.invite_id = $3",
        Filter::Subject(_) => "($2::text IS NULL OR a.subject_hash = $2)",
    };
    let sql = format!(
        "SELECT a.id, EXTRACT(EPOCH FROM a.erased_at)::bigint AS at, a.erased_by, u.name,
                a.exam_id, e.title, a.invite_id, a.subject_hash, a.reason, a.counts
           FROM erasure_audit a
           LEFT JOIN users u ON u.sub = a.erased_by
           LEFT JOIN exams e ON e.id = a.exam_id
          WHERE a.org_id = $1 AND {cond}
          ORDER BY a.erased_at DESC, a.id DESC"
    );
    let query = sqlx::query(&sql).bind(org_id);
    let query = match filter {
        Filter::Id(id) => query.bind(id),
        Filter::Invite(exam_id, invite_id) => query.bind(exam_id).bind(invite_id),
        Filter::Subject(subject) => query.bind(subject),
    };
    let rows = query.fetch_all(pool).await?;
    rows.iter().map(record).collect()
}

fn record(r: &sqlx::postgres::PgRow) -> Result<ErasureRecord, sqlx::Error> {
    let counts: Json<Erased> = r.try_get("counts")?;
    Ok(ErasureRecord {
        id: r.try_get("id")?,
        at: r.try_get("at")?,
        by_sub: r.try_get("erased_by")?,
        by_name: r.try_get("name")?,
        exam_id: r
            .try_get::<Option<Uuid>, _>("exam_id")?
            .map(|id| id.to_string()),
        exam_title: r.try_get("title")?,
        invite_id: r.try_get::<Uuid, _>("invite_id")?.to_string(),
        subject_hash: r.try_get("subject_hash")?,
        reason: r.try_get("reason")?,
        erased: counts.0,
    })
}
