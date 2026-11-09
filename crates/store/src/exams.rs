//! 시험(문항 묶음)·응시 배정 초대·문항 시작 시각(#8). 규칙(용도·제한 범위·상태)은
//! [`promptlab_shared::exam`]에 있고, 여기는 저장만 한다. 시각은 부르는 쪽 시계(유닉스 초)로 받는다
//! — 서버가 제한 시간을 재는 기준이 시험에서도 고정 시계가 되게.

use std::collections::HashMap;

use promptlab_shared::exam::{
    Delivery, ExamItem, ExamSettings, InviteStatus, ItemLimits, Purpose, RubricItem,
};
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::accounts::SecretHash;

fn to_i32(n: u32) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn to_u32(n: i32) -> u32 {
    u32::try_from(n).unwrap_or(0)
}

fn decode_purpose(s: &str) -> Result<Purpose, sqlx::Error> {
    Purpose::parse(s).ok_or_else(|| sqlx::Error::Decode(format!("알 수 없는 용도 {s}").into()))
}

fn decode_delivery(s: &str) -> Result<Delivery, sqlx::Error> {
    Delivery::parse(s)
        .ok_or_else(|| sqlx::Error::Decode(format!("알 수 없는 발송 상태 {s}").into()))
}

/// 시험 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exam {
    pub id: Uuid,
    pub org_id: Uuid,
    pub title: String,
    pub purpose: Purpose,
    pub reveal_scores: bool,
    pub items: Vec<ExamItem>,
    pub created_at: i64,
    /// 대화형 채점 설정(#71).
    pub settings: ExamSettings,
}

/// 새 시험의 내용.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewExam<'a> {
    pub org_id: Uuid,
    pub created_by: &'a str,
    pub title: &'a str,
    pub purpose: Purpose,
    pub reveal_scores: bool,
    pub items: &'a [ExamItem],
    pub settings: &'a ExamSettings,
}

/// 시험과 문항을 한 트랜잭션으로 넣는다.
pub async fn create_exam(pool: &PgPool, exam: &NewExam<'_>, now: i64) -> Result<Uuid, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO exams (org_id, title, purpose, reveal_scores, created_by, created_at,
                            score_weight, rubric, allowed_languages)
         VALUES ($1, $2, $3, $4, $5, to_timestamp($6), $7, $8, $9) RETURNING id",
    )
    .bind(exam.org_id)
    .bind(exam.title)
    .bind(exam.purpose.as_str())
    .bind(exam.reveal_scores)
    .bind(exam.created_by)
    .bind(now as f64)
    .bind(i16::from(exam.settings.score_weight))
    .bind(Json(&exam.settings.rubric))
    .bind(Json(&exam.settings.allowed_languages))
    .fetch_one(&mut *tx)
    .await?;
    for (position, item) in exam.items.iter().enumerate() {
        sqlx::query(
            "INSERT INTO exam_items (exam_id, position, problem_id, time_limit_secs)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .bind(&item.problem_id)
        .bind(to_i32(item.limits.time_limit_secs))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(id)
}

const EXAM_COLUMNS: &str = "id, org_id, title, purpose, reveal_scores,
    score_weight, rubric, allowed_languages,
    EXTRACT(EPOCH FROM created_at)::bigint AS created_at";

/// 시험의 채점 설정을 바꾼다. 시험이 없으면 `false`.
pub async fn set_settings(
    pool: &PgPool,
    exam_id: Uuid,
    settings: &ExamSettings,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE exams SET score_weight = $2, rubric = $3, allowed_languages = $4 WHERE id = $1",
    )
    .bind(exam_id)
    .bind(i16::from(settings.score_weight))
    .bind(Json(&settings.rubric))
    .bind(Json(&settings.allowed_languages))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

fn decode_settings(row: &sqlx::postgres::PgRow) -> Result<ExamSettings, sqlx::Error> {
    let Json(rubric): Json<Vec<RubricItem>> = row.try_get("rubric")?;
    let Json(allowed_languages): Json<Vec<promptlab_shared::coding::Language>> =
        row.try_get("allowed_languages")?;
    Ok(ExamSettings {
        score_weight: u8::try_from(row.try_get::<i16, _>("score_weight")?).unwrap_or(0),
        rubric,
        allowed_languages,
    })
}

async fn items_of(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<ExamItem>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT exam_id, problem_id, time_limit_secs FROM exam_items
          WHERE exam_id = ANY($1) ORDER BY exam_id, position",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut items: HashMap<Uuid, Vec<ExamItem>> = HashMap::new();
    for row in rows {
        items
            .entry(row.try_get("exam_id")?)
            .or_default()
            .push(ExamItem {
                problem_id: row.try_get("problem_id")?,
                limits: ItemLimits {
                    time_limit_secs: to_u32(row.try_get("time_limit_secs")?),
                },
            });
    }
    Ok(items)
}

fn decode_exam(row: &sqlx::postgres::PgRow) -> Result<Exam, sqlx::Error> {
    Ok(Exam {
        id: row.try_get("id")?,
        org_id: row.try_get("org_id")?,
        title: row.try_get("title")?,
        purpose: decode_purpose(row.try_get("purpose")?)?,
        reveal_scores: row.try_get("reveal_scores")?,
        items: Vec::new(),
        created_at: row.try_get("created_at")?,
        settings: decode_settings(row)?,
    })
}

async fn with_items(pool: &PgPool, mut exams: Vec<Exam>) -> Result<Vec<Exam>, sqlx::Error> {
    let ids: Vec<Uuid> = exams.iter().map(|e| e.id).collect();
    let items = items_of(pool, &ids).await?;
    for exam in &mut exams {
        // 같은 시험이 여럿 들어올 수 있다(한 사람의 배정이 같은 시험으로 둘 이상, #39) — 꺼내지 말고 복제한다.
        exam.items = items.get(&exam.id).cloned().unwrap_or_default();
    }
    Ok(exams)
}

/// 조직의 시험들(새것부터).
pub async fn list_exams(pool: &PgPool, org_id: Uuid) -> Result<Vec<Exam>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {EXAM_COLUMNS} FROM exams WHERE org_id = $1 ORDER BY created_at DESC, id"
    ))
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    let exams = rows.iter().map(decode_exam).collect::<Result<_, _>>()?;
    with_items(pool, exams).await
}

/// 그 조직의 시험 하나. 다른 조직의 시험이면 `None`.
pub async fn get_exam(
    pool: &PgPool,
    org_id: Uuid,
    exam_id: Uuid,
) -> Result<Option<Exam>, sqlx::Error> {
    let Some(row) = sqlx::query(&format!(
        "SELECT {EXAM_COLUMNS} FROM exams WHERE org_id = $1 AND id = $2"
    ))
    .bind(org_id)
    .bind(exam_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(with_items(pool, vec![decode_exam(&row)?]).await?.pop())
}

/// 초대를 낸 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteIssue {
    /// 새 토큰으로 초대를 냈다(처음이거나, 아직 받지 않은 초대의 링크를 갈았다 — 옛 링크는 죽는다).
    Issued { id: Uuid, expires_at: i64 },
    /// 이미 누군가 이 초대로 배정을 받았다(링크를 다시 내지 않는다).
    AlreadyAssigned {
        id: Uuid,
        expires_at: i64,
        delivery: Delivery,
    },
}

/// 시험에 이메일 하나를 초대한다. 같은 시험·같은 이메일의 초대는 하나다.
pub async fn issue_invite(
    pool: &PgPool,
    exam_id: Uuid,
    email: &str,
    token_hash: &SecretHash,
    created_by: &str,
    now: i64,
    ttl_secs: i64,
) -> Result<InviteIssue, sqlx::Error> {
    let issued = sqlx::query(
        "INSERT INTO exam_invites (exam_id, email, token_hash, created_by, created_at, expires_at)
         VALUES ($1, $2, $3, $4, to_timestamp($5), to_timestamp($6))
         ON CONFLICT (exam_id, email) DO UPDATE
            SET token_hash = EXCLUDED.token_hash, created_by = EXCLUDED.created_by,
                created_at = EXCLUDED.created_at, expires_at = EXCLUDED.expires_at, delivery = 'pending'
          WHERE exam_invites.candidate_sub IS NULL
        RETURNING id, EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at",
    )
    .bind(exam_id)
    .bind(email)
    .bind(token_hash.as_slice())
    .bind(created_by)
    .bind(now as f64)
    .bind((now + ttl_secs) as f64)
    .fetch_optional(pool)
    .await?;
    if let Some(row) = issued {
        return Ok(InviteIssue::Issued {
            id: row.try_get("id")?,
            expires_at: row.try_get("expires_at")?,
        });
    }
    let row = sqlx::query(
        "SELECT id, EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at, delivery
           FROM exam_invites WHERE exam_id = $1 AND email = $2",
    )
    .bind(exam_id)
    .bind(email)
    .fetch_one(pool)
    .await?;
    Ok(InviteIssue::AlreadyAssigned {
        id: row.try_get("id")?,
        expires_at: row.try_get("expires_at")?,
        delivery: decode_delivery(row.try_get("delivery")?)?,
    })
}

/// 초대 메일 발송 결과를 남긴다.
pub async fn set_delivery(
    pool: &PgPool,
    invite_id: Uuid,
    delivery: Delivery,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE exam_invites SET delivery = $2 WHERE id = $1")
        .bind(invite_id)
        .bind(delivery.as_str())
        .execute(pool)
        .await?;
    Ok(())
}

/// 초대 현황 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteRow {
    pub id: Uuid,
    pub email: String,
    pub status: InviteStatus,
    pub accepted: bool,
    pub candidate_name: Option<String>,
    pub delivery: Delivery,
    pub expires_at: i64,
}

/// 시험의 초대 현황(초대한 순).
pub async fn list_invites(pool: &PgPool, exam_id: Uuid) -> Result<Vec<InviteRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT i.id, i.email, i.delivery, i.candidate_sub IS NOT NULL AS accepted, u.name,
                EXTRACT(EPOCH FROM i.expires_at)::bigint AS expires_at,
                (SELECT count(*) FROM exam_items it WHERE it.exam_id = i.exam_id) AS items,
                (SELECT count(*) FROM exam_item_starts s WHERE s.invite_id = i.id) AS started,
                (SELECT count(*) FROM submissions s WHERE s.invite_id = i.id) AS submitted,
                (SELECT count(*) FROM submissions s
                  WHERE s.invite_id = i.id AND s.status IN ('done', 'failed')) AS finished
           FROM exam_invites i LEFT JOIN users u ON u.sub = i.candidate_sub
          WHERE i.exam_id = $1
          ORDER BY i.created_at, i.email",
    )
    .bind(exam_id)
    .fetch_all(pool)
    .await?;
    let count = |row: &sqlx::postgres::PgRow, name: &str| -> Result<u32, sqlx::Error> {
        Ok(u32::try_from(row.try_get::<i64, _>(name)?).unwrap_or(u32::MAX))
    };
    rows.iter()
        .map(|row| {
            Ok(InviteRow {
                id: row.try_get("id")?,
                email: row.try_get("email")?,
                status: InviteStatus::from_counts(
                    count(row, "items")?,
                    count(row, "started")?,
                    count(row, "submitted")?,
                    count(row, "finished")?,
                ),
                accepted: row.try_get("accepted")?,
                candidate_name: row.try_get("name")?,
                delivery: decode_delivery(row.try_get("delivery")?)?,
                expires_at: row.try_get("expires_at")?,
            })
        })
        .collect()
}

/// 초대 링크로 배정을 받는다. 처음 받는 사람(만료 전)이거나 이미 받은 그 사람이면 배정 id.
/// 없거나 만료됐거나 다른 사람이 받은 초대면 `None`.
pub async fn accept_invite(
    pool: &PgPool,
    token_hash: &SecretHash,
    sub: &str,
    now: i64,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE exam_invites
            SET candidate_sub = $2, accepted_at = COALESCE(accepted_at, to_timestamp($3))
          WHERE token_hash = $1
            AND (candidate_sub = $2 OR (candidate_sub IS NULL AND expires_at > to_timestamp($3)))
        RETURNING id",
    )
    .bind(token_hash.as_slice())
    .bind(sub)
    .bind(now as f64)
    .fetch_optional(pool)
    .await
}

/// 응시자에게 배정된 시험 하나와 문항별 진행.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub id: Uuid,
    pub exam: Exam,
    pub org_name: String,
    /// 문항 번호 → 시작 시각(유닉스 초).
    pub started: HashMap<u32, i64>,
    /// 문항 번호 → 제출 id.
    pub submitted: HashMap<u32, Uuid>,
}

/// 그 사람이 받은 배정들(받은 순). `only`를 주면 그 배정 하나만(그 사람 것이 아니면 빈 목록).
pub async fn assignments_of(
    pool: &PgPool,
    sub: &str,
    only: Option<Uuid>,
) -> Result<Vec<Assignment>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT i.id AS invite_id, o.name AS org_name, e.id, e.org_id, e.title, e.purpose, e.reveal_scores,
                e.score_weight, e.rubric, e.allowed_languages,
                EXTRACT(EPOCH FROM e.created_at)::bigint AS created_at
           FROM exam_invites i JOIN exams e ON e.id = i.exam_id JOIN orgs o ON o.id = e.org_id
          WHERE i.candidate_sub = $1 AND ($2::uuid IS NULL OR i.id = $2)
          ORDER BY i.accepted_at, i.id",
    )
    .bind(sub)
    .bind(only)
    .fetch_all(pool)
    .await?;
    let mut assignments = Vec::with_capacity(rows.len());
    let mut exams = Vec::with_capacity(rows.len());
    for row in &rows {
        exams.push(decode_exam(row)?);
        assignments.push((
            row.try_get::<Uuid, _>("invite_id")?,
            row.try_get::<String, _>("org_name")?,
        ));
    }
    let exams = with_items(pool, exams).await?;
    let ids: Vec<Uuid> = assignments.iter().map(|(id, _)| *id).collect();
    let mut started: HashMap<Uuid, HashMap<u32, i64>> = HashMap::new();
    for row in sqlx::query(
        "SELECT invite_id, position, EXTRACT(EPOCH FROM started_at)::bigint AS started_at
           FROM exam_item_starts WHERE invite_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?
    {
        started
            .entry(row.try_get("invite_id")?)
            .or_default()
            .insert(to_u32(row.try_get("position")?), row.try_get("started_at")?);
    }
    let mut submitted: HashMap<Uuid, HashMap<u32, Uuid>> = HashMap::new();
    for row in sqlx::query(
        "SELECT invite_id, item_position, id FROM submissions WHERE invite_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?
    {
        submitted
            .entry(row.try_get("invite_id")?)
            .or_default()
            .insert(to_u32(row.try_get("item_position")?), row.try_get("id")?);
    }
    Ok(assignments
        .into_iter()
        .zip(exams)
        .map(|((id, org_name), exam)| Assignment {
            id,
            exam,
            org_name,
            started: started.remove(&id).unwrap_or_default(),
            submitted: submitted.remove(&id).unwrap_or_default(),
        })
        .collect())
}

/// 문항 시작 때 신고한 모델 sha256(#131). 시작하지 않았거나 신고가 없으면 `None`.
pub async fn item_model(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT model_sha256 FROM exam_item_starts WHERE invite_id = $1 AND position = $2",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

/// 문항 응시를 시작한다(처음 한 번만 남고, 다시 부르면 처음 시각). 시작 시각(유닉스 초)을 준다.
/// `model_sha256`은 앱이 신고한 모델(#131 「쓴 모델」) — 다시 시작하면 마지막 신고로 바꾼다.
pub async fn start_item(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
    model_sha256: Option<&str>,
    now: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query(
        "INSERT INTO exam_item_starts (invite_id, position, started_at, model_sha256)
         VALUES ($1, $2, to_timestamp($3), $4)
         ON CONFLICT (invite_id, position) DO UPDATE
            SET model_sha256 = COALESCE(EXCLUDED.model_sha256, exam_item_starts.model_sha256)",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .bind(now as f64)
    .bind(model_sha256)
    .execute(pool)
    .await?;
    sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM started_at)::bigint FROM exam_item_starts
          WHERE invite_id = $1 AND position = $2",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_one(pool)
    .await
}

/// 그 제출이 시험 응시면 시험의 용도와 공개 설정.
pub async fn exam_of_invite(
    pool: &PgPool,
    invite_id: Uuid,
) -> Result<Option<(Purpose, bool)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT e.purpose, e.reveal_scores FROM exam_invites i JOIN exams e ON e.id = i.exam_id
          WHERE i.id = $1",
    )
    .bind(invite_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok((
            decode_purpose(row.try_get("purpose")?)?,
            row.try_get("reveal_scores")?,
        ))
    })
    .transpose()
}
