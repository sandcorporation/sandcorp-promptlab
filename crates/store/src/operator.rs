//! 운영자 화면(#19) 저장소: 시범 조직 승인, 큐 지표, 허용 목록 변경 이력. 조직별 비용은 [`crate::usage`],
//! 문제은행은 [`crate::bank`].
//!
//! 스모크 조직(#28)은 목록·지표에서 뺀다(`NOT orgs.is_smoke`). 스모크 조직은 승인과 상관없이 언제나 열려 있다.

use promptlab_shared::integrity::{AllowAction, AllowKind};
use promptlab_shared::operator::{
    AllowlistChange, OperatorOrg, QueueKind, QueueStats, RecentFailure,
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// 운영자가 보는 조직 목록(스모크 제외, 만든 순).
pub async fn orgs(pool: &PgPool) -> Result<Vec<OperatorOrg>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "{ORG_SELECT} WHERE NOT o.is_smoke ORDER BY o.created_at, o.id"
    ))
    .fetch_all(pool)
    .await?;
    rows.iter().map(org_row).collect()
}

const ORG_SELECT: &str = "SELECT o.id, o.name, o.created_by,
        EXTRACT(EPOCH FROM o.created_at)::bigint AS created_at,
        EXTRACT(EPOCH FROM o.approved_at)::bigint AS approved_at,
        (SELECT count(*) FROM memberships m WHERE m.org_id = o.id) AS members,
        (SELECT count(*) FROM exams e WHERE e.org_id = o.id) AS exams
   FROM orgs o";

fn org_row(row: &sqlx::postgres::PgRow) -> Result<OperatorOrg, sqlx::Error> {
    let id: Uuid = row.try_get("id")?;
    Ok(OperatorOrg {
        id: id.to_string(),
        name: row.try_get("name")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        approved_at: row.try_get("approved_at")?,
        members: row.try_get("members")?,
        exams: row.try_get("exams")?,
    })
}

/// 조직을 승인하거나(`approved`) 승인을 거둔다. 없는 조직·스모크 조직은 `None`.
pub async fn set_approval(
    pool: &PgPool,
    org_id: Uuid,
    by: &str,
    approved: bool,
    now: i64,
) -> Result<Option<OperatorOrg>, sqlx::Error> {
    let done = if approved {
        sqlx::query(
            "UPDATE orgs SET approved_at = COALESCE(approved_at, to_timestamp($2)),
                    approved_by = COALESCE(approved_by, $3)
              WHERE id = $1 AND NOT is_smoke",
        )
        .bind(org_id)
        .bind(now as f64)
        .bind(by)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            "UPDATE orgs SET approved_at = NULL, approved_by = NULL WHERE id = $1 AND NOT is_smoke",
        )
        .bind(org_id)
        .execute(pool)
        .await?
    };
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    let row = sqlx::query(&format!("{ORG_SELECT} WHERE o.id = $1"))
        .bind(org_id)
        .fetch_one(pool)
        .await?;
    org_row(&row).map(Some)
}

/// 이 조직이 시험을 열 수 있는가(승인됐거나 스모크 조직). 없는 조직은 `false`.
pub async fn org_open(pool: &PgPool, org_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT approved_at IS NOT NULL OR is_smoke FROM orgs WHERE id = $1",
    )
    .bind(org_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false))
}

/// 큐마다 `st`(waiting·running·done·failed), `enq`(넣은 시각), `started`(시작, 없을 수 있다), `fin`(끝),
/// `org`(조직 이름), `reason`(짧은 실패 이유)을 내는 SELECT. 스모크 조직은 뺀다.
fn queue_source(kind: QueueKind) -> &'static str {
    match kind {
        QueueKind::Grading | QueueKind::PracticeGrading => {
            "SELECT CASE s.status WHEN 'queued' THEN 'waiting' WHEN 'grading' THEN 'running'
                                  ELSE s.status END AS st,
                    s.created_at AS enq, s.started_at AS started, s.finished_at AS fin,
                    o.name AS org, s.failure_reason AS reason, s.practice
               FROM submissions s LEFT JOIN orgs o ON o.id = s.org_id
              WHERE NOT COALESCE(o.is_smoke, false)"
        }
        QueueKind::Preview => {
            "SELECT CASE p.status WHEN 'queued' THEN 'waiting' ELSE p.status END AS st,
                    p.created_at AS enq, NULL::timestamptz AS started, p.finished_at AS fin,
                    coalesce(o.name, '운영자 문항') AS org, p.failure_reason AS reason, false AS practice
               FROM problem_previews p LEFT JOIN orgs o ON o.id = p.org_id
              WHERE NOT COALESCE(o.is_smoke, false)"
        }
        QueueKind::Report => {
            "SELECT CASE r.status WHEN 'queued' THEN 'waiting' WHEN 'generating' THEN 'running'
                                  ELSE r.status END AS st,
                    r.requested_at AS enq, r.started_at AS started, r.finished_at AS fin,
                    o.name AS org, r.failure_reason AS reason, false AS practice
               FROM exam_reports r
               JOIN exam_invites i ON i.id = r.invite_id
               JOIN exams e ON e.id = i.exam_id
               JOIN orgs o ON o.id = e.org_id
              WHERE NOT o.is_smoke"
        }
    }
}

/// 채점 큐는 연습 여부로 둘로 나눈다.
fn queue_filter(kind: QueueKind) -> &'static str {
    match kind {
        QueueKind::Grading => "NOT q.practice",
        QueueKind::PracticeGrading => "q.practice",
        _ => "true",
    }
}

const QUEUES: [QueueKind; 4] = [
    QueueKind::Grading,
    QueueKind::PracticeGrading,
    QueueKind::Preview,
    QueueKind::Report,
];

/// 큐마다 지금 길이와 `now` 전 `window_hours`시간의 처리(끝남·실패·평균 대기·처리 시간).
pub async fn queue_stats(
    pool: &PgPool,
    now: i64,
    window_hours: i64,
) -> Result<Vec<QueueStats>, sqlx::Error> {
    let mut out = Vec::with_capacity(QUEUES.len());
    for kind in QUEUES {
        let sql = format!(
            "WITH q AS ({source}), w AS (SELECT * FROM q WHERE {filter})
             SELECT count(*) FILTER (WHERE st = 'waiting') AS waiting,
                    count(*) FILTER (WHERE st = 'running') AS running,
                    EXTRACT(EPOCH FROM (to_timestamp($1) - min(enq) FILTER (WHERE st = 'waiting')))::bigint
                        AS oldest_waiting,
                    count(*) FILTER (WHERE st = 'done' AND fin >= to_timestamp($2)) AS done,
                    count(*) FILTER (WHERE st = 'failed' AND COALESCE(fin, enq) >= to_timestamp($2)) AS failed,
                    (avg(EXTRACT(EPOCH FROM (COALESCE(started, fin) - enq)))
                        FILTER (WHERE st = 'done' AND fin >= to_timestamp($2)))::float8 AS avg_wait,
                    (avg(EXTRACT(EPOCH FROM (fin - started)))
                        FILTER (WHERE st = 'done' AND fin >= to_timestamp($2) AND started IS NOT NULL))::float8
                        AS avg_run
               FROM w",
            source = queue_source(kind),
            filter = queue_filter(kind),
        );
        let row = sqlx::query(&sql)
            .bind(now as f64)
            .bind((now - window_hours * 3600) as f64)
            .fetch_one(pool)
            .await?;
        out.push(QueueStats {
            kind,
            waiting: row.try_get("waiting")?,
            running: row.try_get("running")?,
            oldest_waiting_secs: row.try_get("oldest_waiting")?,
            done: row.try_get("done")?,
            failed: row.try_get("failed")?,
            avg_wait_secs: row.try_get("avg_wait")?,
            avg_run_secs: row.try_get("avg_run")?,
        });
    }
    Ok(out)
}

/// 최근 실패(새것부터 `limit`개). 이유는 짧은 이유만 — 채점 상세(`failure_detail`)는 읽지 않는다.
pub async fn recent_failures(pool: &PgPool, limit: i64) -> Result<Vec<RecentFailure>, sqlx::Error> {
    let parts: Vec<String> = QUEUES
        .iter()
        .enumerate()
        .map(|(n, kind)| {
            format!(
                "SELECT {n} AS k, org, COALESCE(reason, '') AS reason,
                        EXTRACT(EPOCH FROM COALESCE(fin, enq))::bigint AS at
                   FROM ({source}) q WHERE st = 'failed' AND {filter}",
                source = queue_source(*kind),
                filter = queue_filter(*kind),
            )
        })
        .collect();
    let sql = format!(
        "SELECT * FROM ({}) f ORDER BY at DESC LIMIT $1",
        parts.join(" UNION ALL ")
    );
    let rows = sqlx::query(&sql).bind(limit).fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            let k: i32 = row.try_get("k")?;
            Ok(RecentFailure {
                kind: QUEUES[k as usize],
                org: row.try_get("org")?,
                reason: row.try_get("reason")?,
                at: row.try_get("at")?,
            })
        })
        .collect()
}

/// 값마다 마지막 허용 목록 변경([`promptlab_shared::integrity::Allowlist::with_overrides`]에 넘긴다).
pub async fn allowlist_overrides(
    pool: &PgPool,
) -> Result<Vec<(AllowKind, String, AllowAction)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT DISTINCT ON (kind, value) kind, value, action
           FROM integrity_allowlist_changes ORDER BY kind, value, id DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let kind: String = row.try_get("kind")?;
        let action: String = row.try_get("action")?;
        if let (Some(kind), Some(action)) = (AllowKind::parse(&kind), AllowAction::parse(&action)) {
            out.push((kind, row.try_get("value")?, action));
        }
    }
    Ok(out)
}

/// 허용 목록 변경 이력(새것부터 `limit`개).
pub async fn allowlist_history(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<AllowlistChange>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT kind, value, action, note, changed_by,
                EXTRACT(EPOCH FROM changed_at)::bigint AS changed_at
           FROM integrity_allowlist_changes ORDER BY id DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let kind: String = row.try_get("kind")?;
        let action: String = row.try_get("action")?;
        if let (Some(kind), Some(action)) = (AllowKind::parse(&kind), AllowAction::parse(&action)) {
            out.push(AllowlistChange {
                kind,
                value: row.try_get("value")?,
                action,
                note: row.try_get("note")?,
                changed_by: row.try_get("changed_by")?,
                changed_at: row.try_get("changed_at")?,
            });
        }
    }
    Ok(out)
}

/// 허용 목록 변경을 이력에 덧붙인다(검증은 부르는 쪽이 한다).
pub async fn record_allowlist_change(
    pool: &PgPool,
    kind: AllowKind,
    value: &str,
    action: AllowAction,
    note: &str,
    by: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO integrity_allowlist_changes (kind, value, action, note, changed_by, changed_at)
         VALUES ($1, $2, $3, $4, $5, to_timestamp($6))",
    )
    .bind(kind.as_str())
    .bind(value)
    .bind(action.as_str())
    .bind(note)
    .bind(by)
    .bind(now as f64)
    .execute(pool)
    .await?;
    Ok(())
}
