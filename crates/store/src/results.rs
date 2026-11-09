//! 시험 결과 조회(#10): 응시 현황·점수·언어·통과 수, 같은 구성원의 회차 비교. 읽기만 한다.
//!
//! 싣는 것은 언어·통과 수·통과율·소요 시간뿐이다 — 히든 테스트·코드·`failure_detail`은 읽지도 않는다.
//! 점수는 문항 통과율의 평균이고(실패 문항은 0), 모든 문항의 채점이 끝나기 전에는 없다.

use std::collections::HashMap;

use promptlab_shared::api::{MemberRounds, ResultItem, ResultItemStatus, ResultRow, Round};
use promptlab_shared::coding::Language;
use promptlab_shared::evaluation::{exam_evaluation_score, exam_final_score};
use promptlab_shared::exam::InviteStatus;
use promptlab_shared::grading::GradeReport;
use sqlx::PgPool;
use sqlx::Row;
use sqlx::types::Json;
use uuid::Uuid;

use crate::evaluations::{self, EvalContext, SCORE_COLUMNS};

/// 문항 결과들로 시험 점수를 낸다. 하나라도 채점이 안 끝났으면(또는 문항이 없으면) `None`.
pub fn exam_score(items: &[ResultItem]) -> Option<f64> {
    if items.is_empty() {
        return None;
    }
    let mut sum = 0.0;
    for item in items {
        match item.status {
            ResultItemStatus::Done => sum += item.pass_rate.unwrap_or(0.0),
            ResultItemStatus::Failed => {}
            _ => return None,
        }
    }
    Some(sum / items.len() as f64)
}

fn status_of(items: &[ResultItem]) -> InviteStatus {
    let count = |f: &dyn Fn(ResultItemStatus) -> bool| {
        u32::try_from(items.iter().filter(|i| f(i.status)).count()).unwrap_or(u32::MAX)
    };
    let submitted = count(&|s| {
        matches!(
            s,
            ResultItemStatus::Submitted | ResultItemStatus::Done | ResultItemStatus::Failed
        )
    });
    let finished = count(&|s| matches!(s, ResultItemStatus::Done | ResultItemStatus::Failed));
    let started = count(&|s| s != ResultItemStatus::NotStarted);
    InviteStatus::from_counts(
        u32::try_from(items.len()).unwrap_or(u32::MAX),
        started,
        submitted,
        finished,
    )
}

/// 배정(초대) 하나의 모든 문항 칸.
struct Entry {
    invite_id: Uuid,
    exam_id: Uuid,
    exam_title: String,
    email: String,
    name: Option<String>,
    sub: Option<String>,
    assigned_at: Option<i64>,
    items: Vec<ResultItem>,
}

/// `WHERE` 조건(위치 인자 `$1` 하나)을 받아 초대 × 문항 칸을 읽는다.
async fn load(
    pool: &PgPool,
    filter: &str,
    bind: Filter,
    ctx: EvalContext,
) -> Result<Vec<Entry>, sqlx::Error> {
    let sql = format!(
        "SELECT i.id AS invite_id, i.exam_id, e.title AS exam_title, i.email, u.name,
                i.candidate_sub, EXTRACT(EPOCH FROM i.accepted_at)::bigint AS assigned_at,
                it.position, it.problem_id,
                st.started_at IS NOT NULL AS started, st.model_sha256,
                s.status, s.report, s.language, s.elapsed_secs,
                {SCORE_COLUMNS}
           FROM exam_invites i
           JOIN exams e ON e.id = i.exam_id
           JOIN orgs o ON o.id = e.org_id
           JOIN exam_items it ON it.exam_id = i.exam_id
           LEFT JOIN users u ON u.sub = i.candidate_sub
           LEFT JOIN exam_item_starts st ON st.invite_id = i.id AND st.position = it.position
           LEFT JOIN submissions s ON s.invite_id = i.id AND s.item_position = it.position
           LEFT JOIN submission_evaluations ev ON ev.submission_id = s.id
          WHERE NOT o.is_smoke AND {filter}
          ORDER BY i.created_at, i.email, i.id, it.position"
    );
    let query = sqlx::query(&sql);
    let rows = match bind {
        Filter::Exam(id) | Filter::OrgInternal(id) => query.bind(id),
    }
    .fetch_all(pool)
    .await?;

    let mut entries: Vec<Entry> = Vec::new();
    for row in &rows {
        let invite_id: Uuid = row.try_get("invite_id")?;
        let position: i32 = row.try_get("position")?;
        let status: Option<String> = row.try_get("status")?;
        let started: bool = row.try_get("started")?;
        let report: Option<Json<GradeReport>> = row.try_get("report")?;
        let elapsed: Option<i32> = row.try_get("elapsed_secs")?;
        let language: Option<String> = row.try_get("language")?;
        let to_u32 = |n: i32| u32::try_from(n).unwrap_or(0);
        let item_status = match status.as_deref() {
            Some("done") => ResultItemStatus::Done,
            Some("failed") => ResultItemStatus::Failed,
            Some(_) => ResultItemStatus::Submitted,
            None if started => ResultItemStatus::InProgress,
            None => ResultItemStatus::NotStarted,
        };
        let report = report.map(|Json(r)| r);
        let item = ResultItem {
            position: to_u32(position),
            problem_id: row.try_get("problem_id")?,
            status: item_status,
            language: language.as_deref().and_then(Language::parse),
            pass_rate: report.as_ref().map(GradeReport::pass_rate),
            passed: report.as_ref().map(|r| r.passed),
            total: report.as_ref().map(|r| r.total),
            elapsed_secs: elapsed.map(to_u32),
            score: evaluations::parts_from_row(row)?.map(|p| evaluations::compose(p, ctx)),
            model: row
                .try_get::<Option<String>, _>("model_sha256")?
                .map(|sha| promptlab_shared::inference::model_name(&sha)),
        };
        match entries.last_mut() {
            Some(e) if e.invite_id == invite_id => e.items.push(item),
            _ => entries.push(Entry {
                invite_id,
                exam_id: row.try_get("exam_id")?,
                exam_title: row.try_get("exam_title")?,
                email: row.try_get("email")?,
                name: row.try_get("name")?,
                sub: row.try_get("candidate_sub")?,
                assigned_at: row.try_get("assigned_at")?,
                items: vec![item],
            }),
        }
    }
    Ok(entries)
}

enum Filter {
    Exam(Uuid),
    OrgInternal(Uuid),
}

/// 시험의 응시자별 결과(초대한 순).
pub async fn exam_results(
    pool: &PgPool,
    exam_id: Uuid,
    ctx: EvalContext,
) -> Result<Vec<ResultRow>, sqlx::Error> {
    let entries = load(pool, "i.exam_id = $1", Filter::Exam(exam_id), ctx).await?;
    Ok(entries
        .into_iter()
        .map(|e| ResultRow {
            invite_id: e.invite_id.to_string(),
            email: e.email,
            candidate_name: e.name,
            status: status_of(&e.items),
            score: exam_score(&e.items),
            final_score: exam_final_score(e.items.iter().map(|i| i.score.as_ref())),
            evaluation_score: exam_evaluation_score(e.items.iter().map(|i| i.score.as_ref())),
            items: e.items,
            delete_at: None,
            delete_in_days: None,
        })
        .collect())
}

/// 조직의 사내 진단에서 같은 사람(`sub`)이 배정받은 시험들을 시간순으로. 회차가 하나뿐인 사람도 싣는다.
pub async fn member_rounds(pool: &PgPool, org_id: Uuid) -> Result<Vec<MemberRounds>, sqlx::Error> {
    let entries = load(
        pool,
        "e.org_id = $1 AND e.purpose = 'internal' AND i.candidate_sub IS NOT NULL",
        Filter::OrgInternal(org_id),
        // 회차 비교는 숨은 케이스 점수만 쓴다 — 평가 상태는 읽지 않는다.
        EvalContext {
            enabled: false,
            now: 0,
        },
    )
    .await?;
    let mut by_sub: HashMap<String, MemberRounds> = HashMap::new();
    for e in entries {
        let (Some(sub), Some(assigned_at)) = (e.sub, e.assigned_at) else {
            continue;
        };
        let member = by_sub.entry(sub.clone()).or_insert_with(|| MemberRounds {
            sub,
            name: e.name.clone(),
            rounds: Vec::new(),
        });
        member.rounds.push(Round {
            exam_id: e.exam_id.to_string(),
            exam_title: e.exam_title,
            assigned_at,
            status: status_of(&e.items),
            score: exam_score(&e.items),
        });
    }
    let mut members: Vec<MemberRounds> = by_sub.into_values().collect();
    for m in &mut members {
        m.rounds
            .sort_by(|a, b| (a.assigned_at, &a.exam_id).cmp(&(b.assigned_at, &b.exam_id)));
    }
    members.sort_by(|a, b| (&a.name, &a.sub).cmp(&(&b.name, &b.sub)));
    Ok(members)
}

/// 사내 진단의 응시자 점수 공개를 바꾼다. 바꿨으면 true(채용 시험이면 바꾸지 않고 false).
pub async fn set_reveal_scores(
    pool: &PgPool,
    exam_id: Uuid,
    reveal: bool,
) -> Result<bool, sqlx::Error> {
    let done =
        sqlx::query("UPDATE exams SET reveal_scores = $2 WHERE id = $1 AND purpose = 'internal'")
            .bind(exam_id)
            .bind(reveal)
            .execute(pool)
            .await?;
    Ok(done.rows_affected() == 1)
}
