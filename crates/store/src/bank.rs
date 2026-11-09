//! 운영자 문제은행(#19): 운영자 문항(`bank_problems`)과 내리기(`problem_retirements`).
//!
//! - 내장 문항([`crate::catalog`])은 저장소 파일이 정본이다 — 고치지 않고 **내리기**만 DB에 표시한다.
//! - 운영자 문항은 모든 조직의 문제은행에 들어간다. 연습/시험 구분(`practice`)은 만들 때 정하고 바꾸지 않는다.
//! - 내린 문항은 새 시험·연습 목록·연습 제출에서 빠진다. 문제 **찾기**([`crate::problems::find`]·
//!   [`crate::problems::find_for_org`])는 내린 문항도 찾는다 — 이미 만든 시험의 문항 시작·채점·과거 결과가 깨지지 않게.
//!
//! - 발행 전 문항(#73 — 운영자 문항은 점검·발행 전, 내장은 측정 기준 미달로 내린 것)도 새 시험·연습 목록·연습 제출에서
//!   빠진다([`crate::checks::is_published`]). 찾기는 그대로다.
//!
//! 운영자 문항 원본에는 히든 테스트이 있다 — 서버는 운영자에게만 낸다([`get`]·[`create`]·[`update`]).

use std::borrow::Cow;
use std::collections::HashMap;

use promptlab_shared::authoring::ProblemSource;
use promptlab_shared::coding::{CodingProblem, PublicCodingProblem};
use promptlab_shared::operator::{
    BANK_PROBLEM_PREFIX, BankProblem, BankProblemSummary, ProblemOrigin,
};
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::catalog;
use crate::checks::{self, Scope};

/// 내린 문항 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retirement {
    pub reason: String,
    /// 유닉스 초.
    pub at: i64,
}

/// 내린 문항 전부(id → 내린 정보).
pub async fn retirements(pool: &PgPool) -> Result<HashMap<String, Retirement>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT problem_id, reason, EXTRACT(EPOCH FROM retired_at)::bigint AS at
           FROM problem_retirements",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("problem_id")?,
                Retirement {
                    reason: row.try_get("reason")?,
                    at: row.try_get("at")?,
                },
            ))
        })
        .collect()
}

/// 내린 문항인가.
pub async fn is_retired(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM problem_retirements WHERE problem_id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
}

/// 운영자 문항 하나(내린 것 포함)와 연습 여부. 채점·시험이 [`crate::problems::find`]로 쓴다.
pub(crate) async fn find_operator(
    pool: &PgPool,
    id: &str,
) -> Result<Option<(bool, CodingProblem)>, sqlx::Error> {
    if !id.starts_with(BANK_PROBLEM_PREFIX) {
        return Ok(None);
    }
    let row = sqlx::query("SELECT practice, source FROM bank_problems WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|row| {
        let source: Json<ProblemSource> = row.try_get("source")?;
        Ok((row.try_get("practice")?, source.0.to_problem(id)))
    })
    .transpose()
}

/// 운영자 문항 한 줄.
struct OperatorRow {
    id: String,
    practice: bool,
    source: ProblemSource,
    updated_at: i64,
    published: bool,
}

/// 운영자 문항들(만든 순).
async fn operator_rows(pool: &PgPool) -> Result<Vec<OperatorRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, practice, source, EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at,
                published_at IS NOT NULL AS published
           FROM bank_problems ORDER BY created_at, id",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let source: Json<ProblemSource> = row.try_get("source")?;
            Ok(OperatorRow {
                id: row.try_get("id")?,
                practice: row.try_get("practice")?,
                source: source.0,
                updated_at: row.try_get("updated_at")?,
                published: row.try_get("published")?,
            })
        })
        .collect()
}

/// 지금 쓰는(내리지 않았고 발행한) 문제은행 문항의 공개 보기: `practice`가 참이면 연습 문항, 거짓이면 시험 문항.
/// 내장 먼저, 그다음 운영자 문항(만든 순).
pub async fn active(
    pool: &PgPool,
    practice: bool,
) -> Result<Vec<PublicCodingProblem>, sqlx::Error> {
    let retired = retirements(pool).await?;
    let builtin: Vec<&CodingProblem> = if practice {
        catalog::practice().collect()
    } else {
        catalog::exam_bank().collect()
    };
    let mut out: Vec<PublicCodingProblem> = builtin
        .into_iter()
        .filter(|p| !retired.contains_key(&p.id) && catalog::is_published(&p.id))
        .map(|p| p.public_view())
        .collect();
    for row in operator_rows(pool).await? {
        if row.practice == practice && row.published && !retired.contains_key(&row.id) {
            out.push(row.source.to_problem(&row.id).public_view());
        }
    }
    Ok(out)
}

/// 지금 쓰는 연습 문항 하나(내장·운영자). 시험 문항·내린 문항·발행 전 문항·없는 문항은 `None`.
pub async fn find_practice(
    pool: &PgPool,
    id: &str,
) -> Result<Option<Cow<'static, CodingProblem>>, sqlx::Error> {
    let found = if let Some(problem) = catalog::find(id) {
        catalog::is_practice(id).then_some(Cow::Borrowed(problem))
    } else {
        find_operator(pool, id)
            .await?
            .and_then(|(practice, problem)| practice.then_some(Cow::Owned(problem)))
    };
    match found {
        Some(problem) if !is_retired(pool, id).await? && checks::is_published(pool, id).await? => {
            Ok(Some(problem))
        }
        _ => Ok(None),
    }
}

/// 새 시험에 넣을 수 있는 문항인가: 이 조직이 찾을 수 있고, 연습 문항이 아니고, 내리지 않았고, 발행했다(#73).
pub async fn usable_in_new_exam(
    pool: &PgPool,
    org_id: Uuid,
    id: &str,
) -> Result<bool, sqlx::Error> {
    if catalog::is_practice(id)
        || is_retired(pool, id).await?
        || !checks::is_published(pool, id).await?
    {
        return Ok(false);
    }
    if let Some((practice, _)) = find_operator(pool, id).await? {
        return Ok(!practice);
    }
    Ok(crate::problems::find_for_org(pool, org_id, id)
        .await?
        .is_some())
}

/// 운영자 화면의 문제은행 목록(내장 → 운영자 문항, 히든 테스트 없음).
pub async fn list(pool: &PgPool) -> Result<Vec<BankProblemSummary>, sqlx::Error> {
    let retired = retirements(pool).await?;
    let summary = |id: &str,
                   origin: ProblemOrigin,
                   practice: bool,
                   problem: &CodingProblem,
                   updated_at: Option<i64>,
                   published: bool| {
        let r = retired.get(id);
        BankProblemSummary {
            id: id.to_owned(),
            origin,
            practice,
            title: problem.title.clone(),
            example_count: problem.examples.len(),
            hidden_count: problem.hidden.len(),
            updated_at,
            retired_at: r.map(|r| r.at),
            retired_reason: r.map(|r| r.reason.clone()),
            published,
        }
    };
    let mut out: Vec<BankProblemSummary> = catalog::bank()
        .map(|p| {
            summary(
                &p.id,
                ProblemOrigin::Builtin,
                catalog::is_practice(&p.id),
                p,
                None,
                catalog::is_published(&p.id),
            )
        })
        .collect();
    for row in operator_rows(pool).await? {
        let problem = row.source.to_problem(&row.id);
        out.push(summary(
            &row.id,
            ProblemOrigin::Operator,
            row.practice,
            &problem,
            Some(row.updated_at),
            row.published,
        ));
    }
    Ok(out)
}

/// 문항 하나의 목록 줄(내장·운영자). 없으면 `None`.
pub async fn summary(pool: &PgPool, id: &str) -> Result<Option<BankProblemSummary>, sqlx::Error> {
    Ok(list(pool).await?.into_iter().find(|p| p.id == id))
}

/// 운영자 문항 하나(편집용, 히든 테스트 포함). 내장 문항·없는 문항은 `None`.
pub async fn get(pool: &PgPool, id: &str) -> Result<Option<BankProblem>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT b.practice, b.source, EXTRACT(EPOCH FROM b.updated_at)::bigint AS updated_at,
                EXTRACT(EPOCH FROM r.retired_at)::bigint AS retired_at
           FROM bank_problems b LEFT JOIN problem_retirements r ON r.problem_id = b.id
          WHERE b.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let source: Json<ProblemSource> = row.try_get("source")?;
    let Some(publication) = checks::publication(pool, Scope::Operator, id).await? else {
        return Ok(None);
    };
    Ok(Some(BankProblem {
        id: id.to_owned(),
        practice: row.try_get("practice")?,
        source: source.0,
        updated_at: row.try_get("updated_at")?,
        retired_at: row.try_get("retired_at")?,
        publication,
    }))
}

/// 운영자 문항을 만든다(검증은 부르는 쪽이 한다). `now`는 유닉스 초.
pub async fn create(
    pool: &PgPool,
    by: &str,
    practice: bool,
    source: &ProblemSource,
    now: i64,
) -> Result<BankProblem, sqlx::Error> {
    let id = format!("{BANK_PROBLEM_PREFIX}{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO bank_problems (id, practice, source, created_by, created_at, updated_by, updated_at)
         VALUES ($1, $2, $3, $4, to_timestamp($5), $4, to_timestamp($5))",
    )
    .bind(&id)
    .bind(practice)
    .bind(Json(source))
    .bind(by)
    .bind(now as f64)
    .execute(pool)
    .await?;
    get(pool, &id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// 운영자 문항을 고친다(연습/시험 구분은 그대로). 없는 문항은 `None`.
///
/// 이미 시험에 들어간 문항을 고치면 그 뒤의 채점부터 새 내용을 쓴다(조직 문제와 같다). 유출이면 고치지 말고
/// 새 문항을 만들고 옛 문항을 내린다. 원본이 바뀌면 발행이 풀린다(#73).
pub async fn update(
    pool: &PgPool,
    id: &str,
    by: &str,
    source: &ProblemSource,
    now: i64,
) -> Result<Option<BankProblem>, sqlx::Error> {
    let done = sqlx::query(&format!(
        "UPDATE bank_problems SET {}, source = $2, updated_by = $3, updated_at = to_timestamp($4)
              WHERE id = $1",
        checks::unpublish_on_change("$2")
    ))
    .bind(id)
    .bind(Json(source))
    .bind(by)
    .bind(now as f64)
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(pool, id).await
}

/// 문제은행 문항(내장·운영자)인가.
pub async fn exists(pool: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
    Ok(catalog::find(id).is_some() || find_operator(pool, id).await?.is_some())
}

/// 문항을 내리거나(`retired`) 되살린다. 문제은행에 없는 문항은 `None`.
pub async fn set_retired(
    pool: &PgPool,
    id: &str,
    by: &str,
    retired: bool,
    reason: &str,
    now: i64,
) -> Result<Option<BankProblemSummary>, sqlx::Error> {
    if !exists(pool, id).await? {
        return Ok(None);
    }
    if retired {
        sqlx::query(
            "INSERT INTO problem_retirements (problem_id, reason, retired_by, retired_at)
             VALUES ($1, $2, $3, to_timestamp($4))
             ON CONFLICT (problem_id) DO UPDATE SET reason = EXCLUDED.reason",
        )
        .bind(id)
        .bind(reason)
        .bind(by)
        .bind(now as f64)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("DELETE FROM problem_retirements WHERE problem_id = $1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    summary(pool, id).await
}
