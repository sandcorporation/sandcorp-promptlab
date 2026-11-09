//! 발행 전 점검 큐와 발행 상태(#73) — 조직 문제(#16)·운영자 문항(#19) 둘 다.
//!
//! - 점검 = 저장된 문제 하나의 **그때 원본**(`problem_previews.source` 스냅숏)의 모범 답안을 판정기로 예제 + 히든에
//!   돌린 것. 규칙은 [`promptlab_shared::publication`]. 서버가 [`enqueue`]로 넣고(조직마다 — 운영자 문항은 운영자 전체
//!   — 진행 중 [`MAX_ACTIVE_PREVIEWS`]개), 채점 워커가 **채점 큐가 비었을 때** [`claim_step`]으로 하나 꺼내 판정기에
//!   한 번 돌리고 [`record`]로 붙인다. 워커는 하나라는 전제다.
//! - 발행 = 지금 원본과 같은 원본에 대한 마지막 점검이 모두 통과일 때만([`publish`]). 원본을 고치면 발행이 풀리고
//!   (`problems::update`·`bank::update`) 옛 점검은 `current: false`가 된다.
//! - 발행하지 않은 문제는 새 시험·연습 목록·연습 제출에서 빠진다([`is_published`]). 문제 **찾기**는 그대로 찾는다 —
//!   이미 만든 시험의 문항 시작·채점·과거 결과는 깨지지 않는다(운영자 「내리기」와 같은 원칙).
//! - 내장 문항은 저장소 시험이 점검을 대신한다([`crate::catalog::is_published`]).
//!
//! 점검 결과에는 케이스 입력·출력이 없다(통과 수만).

use promptlab_shared::authoring::{
    MAX_ACTIVE_PREVIEWS, ORG_PROBLEM_PREFIX, PreviewState, PreviewView, ProblemSource, Publication,
};
use promptlab_shared::coding::CodingProblem;
use promptlab_shared::operator::BANK_PROBLEM_PREFIX;
use promptlab_shared::publication::CheckResult;
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use crate::catalog;

/// 점검·발행의 범위: 조직 문제(그 조직) 또는 운영자 문항.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Org(Uuid),
    Operator,
}

impl Scope {
    fn org(self) -> Option<Uuid> {
        match self {
            Scope::Org(id) => Some(id),
            Scope::Operator => None,
        }
    }

    /// 문제 원본 테이블과 그 범위 조건(`$1` = 문제 id, `$2` = 조직 id 또는 NULL).
    fn table(self) -> &'static str {
        match self {
            Scope::Org(_) => "org_problems",
            Scope::Operator => "bank_problems",
        }
    }

    fn owns(self, problem_id: &str) -> bool {
        match self {
            Scope::Org(_) => problem_id.starts_with(ORG_PROBLEM_PREFIX),
            Scope::Operator => problem_id.starts_with(BANK_PROBLEM_PREFIX),
        }
    }
}

/// 문제의 지금 원본(이 범위의 것만).
async fn current_source(
    conn: &mut PgConnection,
    scope: Scope,
    problem_id: &str,
    lock: bool,
) -> Result<Option<ProblemSource>, sqlx::Error> {
    if !scope.owns(problem_id) {
        return Ok(None);
    }
    let sql = match scope {
        Scope::Org(_) => "SELECT source FROM org_problems WHERE id = $1 AND org_id = $2",
        Scope::Operator => "SELECT source FROM bank_problems WHERE id = $1 AND $2::uuid IS NULL",
    };
    let sql = if lock {
        format!("{sql} FOR UPDATE")
    } else {
        sql.to_owned()
    };
    let source: Option<Json<ProblemSource>> = sqlx::query_scalar(&sql)
        .bind(problem_id)
        .bind(scope.org())
        .fetch_optional(conn)
        .await?;
    Ok(source.map(|s| s.0))
}

/// 저장된 문제의 지금 원본으로 점검을 큐에 넣는다. 없는 문제(다른 범위 포함)는 `Ok(Err(NotFound))`, 진행 중이
/// [`MAX_ACTIVE_PREVIEWS`]개면 `Ok(Err(Busy))`.
pub async fn enqueue(
    pool: &PgPool,
    scope: Scope,
    by: &str,
    problem_id: &str,
) -> Result<Result<Uuid, EnqueueError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // 같은 범위의 동시 요청이 상한을 함께 넘지 않게 범위 단위로 줄 세운다.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtext('problem_previews:' || coalesce($1::text, 'operator')))",
    )
    .bind(scope.org())
    .execute(&mut *tx)
    .await?;
    let Some(source) = current_source(&mut tx, scope, problem_id, false).await? else {
        return Ok(Err(EnqueueError::NotFound));
    };
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM problem_previews
          WHERE org_id IS NOT DISTINCT FROM $1 AND status IN ('queued', 'running')",
    )
    .bind(scope.org())
    .fetch_one(&mut *tx)
    .await?;
    if active >= MAX_ACTIVE_PREVIEWS {
        return Ok(Err(EnqueueError::Busy));
    }
    let total = i32::try_from(source.examples.len() + source.hidden.len()).unwrap_or(i32::MAX);
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO problem_previews (org_id, requested_by, problem_id, source, total)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(scope.org())
    .bind(by)
    .bind(problem_id)
    .bind(Json(&source))
    .bind(total)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(id))
}

/// [`enqueue`]가 넣지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueError {
    NotFound,
    Busy,
}

/// 점검 줄을 고르는 SELECT(문제 원본 테이블과 이어 `current`를 낸다).
fn select_sql(scope: Scope, filter: &str) -> String {
    format!(
        "SELECT p.id, p.problem_id, p.status, p.source, p.result, p.failure_reason,
                EXTRACT(EPOCH FROM p.created_at)::bigint AS created_at,
                coalesce(t.source = p.source, false) AS current,
                CASE WHEN p.status = 'queued' THEN
                    (SELECT count(*) FROM submissions s WHERE s.status IN ('queued', 'grading'))
                  + (SELECT count(*) FROM problem_previews q
                      WHERE q.status IN ('queued', 'running') AND q.created_at < p.created_at)
                END AS queue_ahead
           FROM problem_previews p LEFT JOIN {table} t ON t.id = p.problem_id
          WHERE p.org_id IS NOT DISTINCT FROM $2 AND p.problem_id LIKE $3 AND {filter}",
        table = scope.table()
    )
}

fn prefix_like(scope: Scope) -> String {
    match scope {
        Scope::Org(_) => format!("{ORG_PROBLEM_PREFIX}%"),
        Scope::Operator => format!("{BANK_PROBLEM_PREFIX}%"),
    }
}

fn to_view(row: &PgRow) -> Result<PreviewView, sqlx::Error> {
    let id: Uuid = row.try_get("id")?;
    let status: String = row.try_get("status")?;
    let source: Json<ProblemSource> = row.try_get("source")?;
    let result: Option<Json<CheckResult>> = row.try_get("result")?;
    let mut view = PreviewView {
        id: id.to_string(),
        problem_id: row.try_get("problem_id")?,
        state: match status.as_str() {
            "queued" => PreviewState::Queued,
            "running" => PreviewState::Running,
            "done" => PreviewState::Done,
            _ => PreviewState::Failed,
        },
        created_at: row.try_get("created_at")?,
        current: row.try_get("current")?,
        example_count: source.0.examples.len(),
        hidden_count: source.0.hidden.len(),
        result: result.map(|j| j.0),
        publishable: false,
        reason: None,
        queue_ahead: row.try_get("queue_ahead")?,
        failure_reason: row.try_get("failure_reason")?,
    };
    view.judge();
    Ok(view)
}

/// 점검 하나(이 범위의 것만).
pub async fn get(
    pool: &PgPool,
    scope: Scope,
    id: Uuid,
) -> Result<Option<PreviewView>, sqlx::Error> {
    let row = sqlx::query(&select_sql(scope, "p.id = $1"))
        .bind(id)
        .bind(scope.org())
        .bind(prefix_like(scope))
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(to_view).transpose()
}

/// 문제의 지금 원본에 대한 마지막 점검.
async fn latest_current(
    conn: &mut PgConnection,
    scope: Scope,
    problem_id: &str,
) -> Result<Option<PreviewView>, sqlx::Error> {
    let sql = format!(
        "{} ORDER BY p.created_at DESC, p.id DESC LIMIT 1",
        select_sql(scope, "p.problem_id = $1 AND t.source = p.source")
    );
    let row = sqlx::query(&sql)
        .bind(problem_id)
        .bind(scope.org())
        .bind(prefix_like(scope))
        .fetch_optional(conn)
        .await?;
    row.as_ref().map(to_view).transpose()
}

/// 문제의 발행 상태(이 범위의 문제가 아니면 `None`).
pub async fn publication(
    pool: &PgPool,
    scope: Scope,
    problem_id: &str,
) -> Result<Option<Publication>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    publication_in(&mut conn, scope, problem_id).await
}

async fn publication_in(
    conn: &mut PgConnection,
    scope: Scope,
    problem_id: &str,
) -> Result<Option<Publication>, sqlx::Error> {
    if !scope.owns(problem_id) {
        return Ok(None);
    }
    let sql = format!(
        "SELECT EXTRACT(EPOCH FROM published_at)::bigint AS published_at
           FROM {} WHERE id = $1 AND {}",
        scope.table(),
        match scope {
            Scope::Org(_) => "org_id = $2",
            Scope::Operator => "$2::uuid IS NULL",
        }
    );
    let row = sqlx::query(&sql)
        .bind(problem_id)
        .bind(scope.org())
        .fetch_optional(&mut *conn)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let published_at: Option<i64> = row.try_get("published_at")?;
    Ok(Some(Publication {
        published: published_at.is_some(),
        published_at,
        last_check: latest_current(conn, scope, problem_id).await?,
    }))
}

/// [`publish`]의 결과.
#[derive(Debug, Clone, PartialEq)]
pub enum PublishOutcome {
    /// 이 범위에 없는 문제.
    NotFound,
    /// 지금 원본으로 점검한 적이 없다(점검 안 함 또는 고친 뒤 재검사 전).
    NotChecked,
    /// 마지막 점검이 끝나지 않았거나 통과하지 못했다.
    NotPassed(Box<PreviewView>),
    Published(Box<Publication>),
}

/// 발행한다: 지금 원본과 같은 원본에 대한 마지막 점검이 모두 통과여야 한다. 이미 발행했으면 그대로 돌려준다.
pub async fn publish(
    pool: &PgPool,
    scope: Scope,
    problem_id: &str,
    by: &str,
    now: i64,
) -> Result<PublishOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // 원본을 잠가 점검 판정과 발행 사이에 고치지 못하게 한다.
    if current_source(&mut tx, scope, problem_id, true)
        .await?
        .is_none()
    {
        return Ok(PublishOutcome::NotFound);
    }
    let Some(check) = latest_current(&mut tx, scope, problem_id).await? else {
        return Ok(PublishOutcome::NotChecked);
    };
    if !check.publishable {
        return Ok(PublishOutcome::NotPassed(Box::new(check)));
    }
    let sql = format!(
        "UPDATE {} SET published_at = coalesce(published_at, to_timestamp($3)),
                       published_by = coalesce(published_by, $4)
          WHERE id = $1 AND {}",
        scope.table(),
        match scope {
            Scope::Org(_) => "org_id = $2",
            Scope::Operator => "$2::uuid IS NULL",
        }
    );
    sqlx::query(&sql)
        .bind(problem_id)
        .bind(scope.org())
        .bind(now as f64)
        .bind(by)
        .execute(&mut *tx)
        .await?;
    let publication = publication_in(&mut tx, scope, problem_id).await?;
    tx.commit().await?;
    Ok(publication.map_or(PublishOutcome::NotFound, |p| {
        PublishOutcome::Published(Box::new(p))
    }))
}

/// 고칠 때 발행을 푸는 SET 조각(원본이 바뀌었을 때만 — 같은 원본을 다시 저장하면 그대로). `$new`는 새 원본 자리.
pub(crate) fn unpublish_on_change(new: &str) -> String {
    format!(
        "published_at = CASE WHEN source = {new} THEN published_at END,
         published_by = CASE WHEN source = {new} THEN published_by END"
    )
}

/// 발행한 문제인가(새 시험·연습 목록·연습 제출에 쓸 수 있는가). 내장은 늘 발행, 운영자 문항·조직
/// 문제는 발행 표시. 없는 문제는 `false`.
pub async fn is_published(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
    if catalog::find(id).is_some() {
        return Ok(catalog::is_published(id));
    }
    let sql = if id.starts_with(BANK_PROBLEM_PREFIX) {
        "SELECT published_at IS NOT NULL FROM bank_problems WHERE id = $1"
    } else if id.starts_with(ORG_PROBLEM_PREFIX) {
        "SELECT published_at IS NOT NULL FROM org_problems WHERE id = $1"
    } else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar(sql)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(false))
}

/// 워커가 돌릴 점검 하나: 그때 원본의 문제(모범 답안·예제·히든 포함).
#[derive(Debug)]
pub struct CheckStep {
    pub id: Uuid,
    pub problem: CodingProblem,
}

/// 가장 오래된 대기 점검을 꺼낸다(상태를 `running`으로). 워커가 도중에 죽어 `running`에 남은 것도 다시 꺼낸다.
pub async fn claim_step(pool: &PgPool) -> Result<Option<CheckStep>, sqlx::Error> {
    let row = sqlx::query(
        "UPDATE problem_previews SET status = 'running'
          WHERE id = (
                SELECT id FROM problem_previews
                 WHERE status IN ('queued', 'running')
                 ORDER BY created_at, id
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1)
        RETURNING id, problem_id, source",
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let problem_id: String = row.try_get("problem_id")?;
    let source: Json<ProblemSource> = row.try_get("source")?;
    Ok(Some(CheckStep {
        id: row.try_get("id")?,
        problem: source.0.to_problem(&problem_id),
    }))
}

/// 판정 결과를 붙이고 끝낸다.
pub async fn record(pool: &PgPool, id: Uuid, result: &CheckResult) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE problem_previews SET result = $2, status = 'done', finished_at = now()
          WHERE id = $1 AND status = 'running'",
    )
    .bind(id)
    .bind(Json(result))
    .execute(pool)
    .await?;
    Ok(())
}

/// 점검을 큐로 되돌린다(판정기 일시 오류 — 다음 꺼냄에서 다시 돈다).
pub async fn requeue(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE problem_previews SET status = 'queued' WHERE id = $1 AND status = 'running'",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 점검을 실패로 끝낸다(짧은 까닭만 — 케이스 내용·컴파일 메시지를 싣지 않는다).
pub async fn fail(pool: &PgPool, id: Uuid, reason: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE problem_previews SET status = 'failed', failure_reason = $2, finished_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}
