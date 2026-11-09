//! 토큰·비용 기록(#15): 모델 호출 하나에 한 줄(`usage_records`), 조직·시험·응시로 묶어 본다.
//!
//! 무엇을 기록하나: Claude 호출(AI 리포트·AI 활용 평가). 판정기 실행(채점·발행 전 점검)은 토큰이 없어 남기지 않는다
//! (spec #93).
//! 단가는 기록 시점 값을 함께 남긴다. 집계는 스모크 조직을 뺀다(`NOT orgs.is_smoke`).

use std::collections::BTreeMap;

use promptlab_shared::operator::OrgCost;
use promptlab_shared::report::{
    ExamUsage, KindUsage, OrgUsage, TokenUsage, UnitPrices, UsageKind, UsageTotals,
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// 기록을 무엇에 붙이나. 조직·시험·응시는 저장할 때 이것에서 찾는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageScope {
    /// 제출 하나(시험 응시면 그 초대·시험·조직, 연습 제출이면 조직 없음).
    Submission(Uuid),
    /// 응시(초대) 하나.
    Invite(Uuid),
}

/// 호출 하나의 기록.
#[derive(Debug, Clone, PartialEq)]
pub struct NewUsage<'a> {
    pub kind: UsageKind,
    pub scope: UsageScope,
    pub model: &'a str,
    pub usage: TokenUsage,
    pub duration_ms: u64,
    pub prices: UnitPrices,
}

fn to_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn to_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// 한 줄을 남긴다. 추정 비용 = 기록 시점 단가 × 토큰.
pub async fn record(pool: &PgPool, new: &NewUsage<'_>) -> Result<(), sqlx::Error> {
    // 범위에서 조직·시험·응시·제출·미리 돌리기를 찾는 SELECT. 범위 행이 없어도(지워졌어도) 기록은 남긴다.
    let scope_sql = match new.scope {
        UsageScope::Submission(_) => {
            "SELECT e.org_id, e.id AS exam_id, i.id AS invite_id, s.id AS submission_id, NULL::uuid AS preview_id
               FROM (SELECT $1::uuid AS id) k
               LEFT JOIN submissions s ON s.id = k.id
               LEFT JOIN exam_invites i ON i.id = s.invite_id
               LEFT JOIN exams e ON e.id = i.exam_id"
        }
        UsageScope::Invite(_) => {
            "SELECT e.org_id, e.id AS exam_id, i.id AS invite_id, NULL::uuid AS submission_id, NULL::uuid AS preview_id
               FROM (SELECT $1::uuid AS id) k
               LEFT JOIN exam_invites i ON i.id = k.id
               LEFT JOIN exams e ON e.id = i.exam_id"
        }
    };
    let id = match new.scope {
        UsageScope::Submission(id) | UsageScope::Invite(id) => id,
    };
    let sql = format!(
        "INSERT INTO usage_records
            (kind, org_id, exam_id, invite_id, submission_id, preview_id, model,
             input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, duration_ms,
             price_input, price_output, price_cache_read, price_cache_write, cost_usd)
         SELECT $2, org_id, exam_id, invite_id, submission_id, preview_id, $3,
                $4, $5, $6, $7, $8, $9, $10, $11, $12, $13
           FROM ({scope_sql}) scope"
    );
    // 범위 행을 읽은 뒤 그 행이 지워지면(관리자 삭제 #55·보관 기간 삭제 #54가 채점·리포트와 겹침) 넣을 때 FK가 깨진다.
    // 한 번 더 넣으면 이번에는 범위 행이 없어 연결 없이(숫자만) 남는다.
    for attempt in 0..2 {
        let done = sqlx::query(&sql)
            .bind(id)
            .bind(new.kind.as_str())
            .bind(new.model)
            .bind(to_i64(new.usage.input_tokens))
            .bind(to_i64(new.usage.output_tokens))
            .bind(to_i64(new.usage.cache_read_tokens))
            .bind(to_i64(new.usage.cache_write_tokens))
            .bind(to_i64(new.duration_ms))
            .bind(new.prices.input)
            .bind(new.prices.output)
            .bind(new.prices.cache_read)
            .bind(new.prices.cache_write)
            .bind(new.prices.cost_usd(&new.usage))
            .execute(pool)
            .await;
        match done {
            Err(sqlx::Error::Database(e)) if attempt == 0 && e.is_foreign_key_violation() => {}
            other => return other.map(|_| ()),
        }
    }
    Ok(())
}

const SUMS: &str = "count(*) AS calls,
        COALESCE(sum(u.input_tokens), 0)::bigint AS input_tokens,
        COALESCE(sum(u.output_tokens), 0)::bigint AS output_tokens,
        COALESCE(sum(u.cache_read_tokens), 0)::bigint AS cache_read_tokens,
        COALESCE(sum(u.cache_write_tokens), 0)::bigint AS cache_write_tokens,
        COALESCE(sum(u.duration_ms), 0)::bigint AS duration_ms,
        COALESCE(sum(u.cost_usd), 0)::double precision AS cost_usd";

fn totals(row: &sqlx::postgres::PgRow) -> Result<UsageTotals, sqlx::Error> {
    Ok(UsageTotals {
        calls: to_u64(row.try_get("calls")?),
        input_tokens: to_u64(row.try_get("input_tokens")?),
        output_tokens: to_u64(row.try_get("output_tokens")?),
        cache_read_tokens: to_u64(row.try_get("cache_read_tokens")?),
        cache_write_tokens: to_u64(row.try_get("cache_write_tokens")?),
        duration_ms: to_u64(row.try_get("duration_ms")?),
        cost_usd: row.try_get("cost_usd")?,
    })
}

/// 종류 순서(채점 → 리포트 → 미리 돌리기 → 대조)로 늘어놓는다. 기록이 없는 종류는 뺀다.
fn ordered(map: BTreeMap<&'static str, UsageTotals>) -> Vec<KindUsage> {
    UsageKind::ALL
        .into_iter()
        .filter_map(|kind| {
            map.get(kind.as_str()).map(|totals| KindUsage {
                kind,
                totals: totals.clone(),
            })
        })
        .collect()
}

fn kind_key(kind: &str) -> Option<&'static str> {
    UsageKind::parse(kind).map(UsageKind::as_str)
}

/// 조직의 응시 수·토큰·추정 비용(spec 21). 스모크 조직이면 비어 있다.
pub async fn org_usage(pool: &PgPool, org_id: Uuid) -> Result<OrgUsage, sqlx::Error> {
    // 조직 전체 종류별.
    let rows = sqlx::query(&format!(
        "SELECT u.kind, {SUMS}
           FROM usage_records u JOIN orgs o ON o.id = u.org_id
          WHERE u.org_id = $1 AND NOT o.is_smoke
          GROUP BY u.kind"
    ))
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    let mut org_kinds = BTreeMap::new();
    for row in &rows {
        let kind: String = row.try_get("kind")?;
        if let Some(key) = kind_key(&kind) {
            org_kinds.insert(key, totals(row)?);
        }
    }

    // 시험별 응시 수(배정을 받은 초대 + 보관 기간이 지나 지운 응시의 익명 숫자, #54).
    let exams = sqlx::query(
        "SELECT e.id, e.title,
                (SELECT count(*) FROM exam_invites i
                  WHERE i.exam_id = e.id AND i.candidate_sub IS NOT NULL)
                  + e.erased_attempts AS attempts
           FROM exams e JOIN orgs o ON o.id = e.org_id
          WHERE e.org_id = $1 AND NOT o.is_smoke
          ORDER BY e.created_at DESC, e.id",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;

    // 시험별 종류별.
    let rows = sqlx::query(&format!(
        "SELECT u.exam_id, u.kind, {SUMS}
           FROM usage_records u JOIN orgs o ON o.id = u.org_id
          WHERE u.org_id = $1 AND NOT o.is_smoke AND u.exam_id IS NOT NULL
          GROUP BY u.exam_id, u.kind"
    ))
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    let mut by_exam: BTreeMap<Uuid, BTreeMap<&'static str, UsageTotals>> = BTreeMap::new();
    for row in &rows {
        let exam_id: Uuid = row.try_get("exam_id")?;
        let kind: String = row.try_get("kind")?;
        if let Some(key) = kind_key(&kind) {
            by_exam
                .entry(exam_id)
                .or_default()
                .insert(key, totals(row)?);
        }
    }

    let mut attempts = 0;
    let mut exam_rows = Vec::with_capacity(exams.len());
    for row in exams {
        let id: Uuid = row.try_get("id")?;
        let count = to_u64(row.try_get("attempts")?);
        attempts += count;
        exam_rows.push(ExamUsage {
            exam_id: id.to_string(),
            title: row.try_get("title")?,
            attempts: count,
            by_kind: ordered(by_exam.remove(&id).unwrap_or_default()),
        });
    }
    let by_kind = ordered(org_kinds);
    let cost_usd = by_kind.iter().fold(0.0, |sum, k| sum + k.totals.cost_usd);
    Ok(OrgUsage {
        attempts,
        by_kind,
        cost_usd,
        exams: exam_rows,
    })
}

/// 운영자 화면(#19): 조직마다 응시 수·종류별 토큰·추정 비용(위 [`org_usage`]와 같은 합), 비용 큰 순.
/// 스모크 조직은 빼고, 조직 없는 개인 연습(#18) 채점은 마지막에 「개인 연습」 한 줄로 따로 싣는다(기록이 있을 때).
pub async fn operator_costs(pool: &PgPool) -> Result<Vec<OrgCost>, sqlx::Error> {
    let orgs = sqlx::query(
        "SELECT o.id, o.name,
                (SELECT count(*) FROM exam_invites i JOIN exams e ON e.id = i.exam_id
                  WHERE e.org_id = o.id AND i.candidate_sub IS NOT NULL)
                  + (SELECT COALESCE(sum(e.erased_attempts), 0) FROM exams e WHERE e.org_id = o.id)
                  AS attempts
           FROM orgs o WHERE NOT o.is_smoke ORDER BY o.created_at, o.id",
    )
    .fetch_all(pool)
    .await?;
    let rows = sqlx::query(&format!(
        "SELECT u.org_id, u.kind, {SUMS}
           FROM usage_records u JOIN orgs o ON o.id = u.org_id
          WHERE NOT o.is_smoke
          GROUP BY u.org_id, u.kind"
    ))
    .fetch_all(pool)
    .await?;
    let mut by_org: BTreeMap<Uuid, BTreeMap<&'static str, UsageTotals>> = BTreeMap::new();
    for row in &rows {
        let org_id: Uuid = row.try_get("org_id")?;
        let kind: String = row.try_get("kind")?;
        if let Some(key) = kind_key(&kind) {
            by_org.entry(org_id).or_default().insert(key, totals(row)?);
        }
    }
    let mut out = Vec::with_capacity(orgs.len() + 1);
    for row in orgs {
        let id: Uuid = row.try_get("id")?;
        let by_kind = ordered(by_org.remove(&id).unwrap_or_default());
        out.push(OrgCost {
            org_id: Some(id.to_string()),
            name: row.try_get("name")?,
            attempts: to_u64(row.try_get("attempts")?),
            cost_usd: by_kind.iter().fold(0.0, |sum, k| sum + k.totals.cost_usd),
            by_kind,
        });
    }
    out.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd));

    // 개인 연습: 조직 없는 연습 제출에 붙은 기록.
    let rows = sqlx::query(&format!(
        "SELECT u.kind, {SUMS}
           FROM usage_records u JOIN submissions s ON s.id = u.submission_id
          WHERE u.org_id IS NULL AND s.practice
          GROUP BY u.kind"
    ))
    .fetch_all(pool)
    .await?;
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM submissions WHERE practice")
        .fetch_one(pool)
        .await?;
    if !rows.is_empty() || attempts > 0 {
        let mut kinds = BTreeMap::new();
        for row in &rows {
            let kind: String = row.try_get("kind")?;
            if let Some(key) = kind_key(&kind) {
                kinds.insert(key, totals(row)?);
            }
        }
        let by_kind = ordered(kinds);
        out.push(OrgCost {
            org_id: None,
            name: PRACTICE_ROW.to_owned(),
            attempts: to_u64(attempts),
            cost_usd: by_kind.iter().fold(0.0, |sum, k| sum + k.totals.cost_usd),
            by_kind,
        });
    }
    Ok(out)
}

/// 운영자 비용 표에서 개인 연습 묶음의 이름.
pub const PRACTICE_ROW: &str = "개인 연습";
