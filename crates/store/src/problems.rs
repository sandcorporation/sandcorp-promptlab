//! 문제 찾기(내장 문제은행 + 운영자 문항 + 조직 문제)와 조직 문제 저장(#16). 문제는 모두 코딩 문제(spec #93).
//!
//! - 채점·시험은 [`find`]·[`find_for_org`]로 문제를 찾는다: 내장 문제은행([`crate::catalog`])을 먼저, 없으면
//!   `org_problems`. 조직 문제는 **그 조직의 시험에서만** 쓴다([`find_for_org`]).
//! - 발행 전 점검 큐·발행 상태(#73)는 [`crate::checks`]. 발행하지 않은 조직 문제는 시험 문제은행([`bank`])에서 빠진다.
//!
//! 원본(`source`)에는 히든 테스트·모범 답안이 있다 — 서버는 자기 조직의 시험 관리 역할에게만 낸다.

use std::borrow::Cow;

use promptlab_shared::authoring::{
    ORG_PROBLEM_PREFIX, OrgProblem, OrgProblemSummary, ProblemSource,
};
use promptlab_shared::coding::{CodingProblem, PublicCodingProblem};
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::catalog;
use crate::checks::{self, Scope};

/// id로 문제 하나(내장 → 운영자 문항(#19) → 조직 문제). 채점 워커가 쓴다. 내린 문항도 찾는다.
pub async fn find(
    pool: &PgPool,
    id: &str,
) -> Result<Option<Cow<'static, CodingProblem>>, sqlx::Error> {
    if let Some(problem) = catalog::find(id) {
        return Ok(Some(Cow::Borrowed(problem)));
    }
    if let Some((_, problem)) = crate::bank::find_operator(pool, id).await? {
        return Ok(Some(Cow::Owned(problem)));
    }
    let row = sqlx::query("SELECT source FROM org_problems WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|r| to_problem(id, r.try_get("source")?))
        .transpose()
        .map(|p| p.map(Cow::Owned))
}

/// 이 조직이 쓸 수 있는 문제 하나(내장·운영자 문제은행 또는 이 조직의 문제). 다른 조직의 문제는 `None`.
/// 내린 문항(#19)도 찾는다 — 이미 만든 시험이 깨지지 않게. 새 시험은 [`crate::bank::usable_in_new_exam`]으로 거른다.
pub async fn find_for_org(
    pool: &PgPool,
    org_id: Uuid,
    id: &str,
) -> Result<Option<Cow<'static, CodingProblem>>, sqlx::Error> {
    if let Some(problem) = catalog::find(id) {
        return Ok(Some(Cow::Borrowed(problem)));
    }
    if let Some((_, problem)) = crate::bank::find_operator(pool, id).await? {
        return Ok(Some(Cow::Owned(problem)));
    }
    let row = sqlx::query("SELECT source FROM org_problems WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org_id)
        .fetch_optional(pool)
        .await?;
    row.map(|r| to_problem(id, r.try_get("source")?))
        .transpose()
        .map(|p| p.map(Cow::Owned))
}

fn to_problem(id: &str, source: Json<ProblemSource>) -> Result<CodingProblem, sqlx::Error> {
    Ok(source.0.to_problem(id))
}

/// 시험에 넣을 수 있는 문제(공개 보기): 문제은행(내장 + 운영자 문항, 연습 문제(#18)·내린 문항(#19)·발행 전 문항(#73)
/// 제외) + 이 조직의 발행한 문제(최근 고친 것 먼저).
pub async fn bank(pool: &PgPool, org_id: Uuid) -> Result<Vec<PublicCodingProblem>, sqlx::Error> {
    let mut out: Vec<PublicCodingProblem> = crate::bank::active(pool, false).await?;
    let rows = sqlx::query(
        "SELECT id, source FROM org_problems WHERE org_id = $1 AND published_at IS NOT NULL
          ORDER BY updated_at DESC, id",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let source: Json<ProblemSource> = row.try_get("source")?;
        out.push(source.0.to_problem(&id).public_view());
    }
    Ok(out)
}

/// 조직 문제 목록(히든 테스트 없음).
pub async fn list(pool: &PgPool, org_id: Uuid) -> Result<Vec<OrgProblemSummary>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, source, EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at,
                published_at IS NOT NULL AS published
           FROM org_problems WHERE org_id = $1 ORDER BY updated_at DESC, id",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let source: Json<ProblemSource> = row.try_get("source")?;
            Ok(OrgProblemSummary {
                id: row.try_get("id")?,
                title: source.0.title.trim().to_owned(),
                example_count: source.0.examples.len(),
                hidden_count: source.0.hidden.len(),
                updated_at: row.try_get("updated_at")?,
                published: row.try_get("published")?,
            })
        })
        .collect()
}

/// 조직 문제 하나(편집용, 히든 테스트 포함). 다른 조직의 문제는 `None`.
pub async fn get(pool: &PgPool, org_id: Uuid, id: &str) -> Result<Option<OrgProblem>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT source, EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at
           FROM org_problems WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let source: Json<ProblemSource> = row.try_get("source")?;
    let Some(publication) = checks::publication(pool, Scope::Org(org_id), id).await? else {
        return Ok(None);
    };
    Ok(Some(OrgProblem {
        id: id.to_owned(),
        source: source.0,
        updated_at: row.try_get("updated_at")?,
        publication,
    }))
}

/// 조직 문제를 만든다(검증은 부르는 쪽이 한다). `now`는 유닉스 초.
pub async fn create(
    pool: &PgPool,
    org_id: Uuid,
    by: &str,
    source: &ProblemSource,
    now: i64,
) -> Result<OrgProblem, sqlx::Error> {
    let id = format!("{ORG_PROBLEM_PREFIX}{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO org_problems (id, org_id, source, created_by, created_at, updated_by, updated_at)
         VALUES ($1, $2, $3, $4, to_timestamp($5), $4, to_timestamp($5))",
    )
    .bind(&id)
    .bind(org_id)
    .bind(Json(source))
    .bind(by)
    .bind(now as f64)
    .execute(pool)
    .await?;
    get(pool, org_id, &id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

/// 조직 문제를 고친다. 다른 조직의 문제·없는 문제는 `None`.
///
/// 이미 시험에 들어간 문제를 고치면 그 뒤의 채점부터 새 내용을 쓴다(판 관리는 뒤 티켓). 원본이 바뀌면 발행이 풀린다
/// (#73 — 다시 점검·발행해야 새 시험에 넣는다).
pub async fn update(
    pool: &PgPool,
    org_id: Uuid,
    id: &str,
    by: &str,
    source: &ProblemSource,
    now: i64,
) -> Result<Option<OrgProblem>, sqlx::Error> {
    let done = sqlx::query(&format!(
        "UPDATE org_problems SET {}, source = $3, updated_by = $4, updated_at = to_timestamp($5)
              WHERE id = $1 AND org_id = $2",
        checks::unpublish_on_change("$3")
    ))
    .bind(id)
    .bind(org_id)
    .bind(Json(source))
    .bind(by)
    .bind(now as f64)
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(pool, org_id, id).await
}
