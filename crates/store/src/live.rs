//! 채용 시험의 응시 가능 시간(#14). 규칙은 [`promptlab_shared::live`].
//!
//! 실시간 보기 시그널링은 서버 메모리에만 둔다 — 여기에는 시간 창만 있다.

use promptlab_shared::exam::Purpose;
use promptlab_shared::live::ExamWindow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const WINDOW_COLUMNS: &str = "e.purpose, e.window_open,
    EXTRACT(EPOCH FROM e.window_starts_at)::bigint AS starts_at,
    EXTRACT(EPOCH FROM e.window_ends_at)::bigint AS ends_at";

fn decode(row: &sqlx::postgres::PgRow) -> Result<(Purpose, ExamWindow), sqlx::Error> {
    let purpose: String = row.try_get("purpose")?;
    let purpose = Purpose::parse(&purpose)
        .ok_or_else(|| sqlx::Error::Decode(format!("알 수 없는 용도 {purpose}").into()))?;
    Ok((
        purpose,
        ExamWindow {
            open: row.try_get("window_open")?,
            starts_at: row.try_get("starts_at")?,
            ends_at: row.try_get("ends_at")?,
        },
    ))
}

/// 시험의 용도와 응시 가능 시간.
pub async fn window_of_exam(
    pool: &PgPool,
    exam_id: Uuid,
) -> Result<Option<(Purpose, ExamWindow)>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT {WINDOW_COLUMNS} FROM exams e WHERE e.id = $1"
    ))
    .bind(exam_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(decode).transpose()
}

/// 배정(초대)이 속한 시험의 용도·응시 가능 시간과 스모크 조직 여부.
pub async fn window_of_invite(
    pool: &PgPool,
    invite_id: Uuid,
) -> Result<Option<(Purpose, ExamWindow, bool)>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT {WINDOW_COLUMNS}, o.is_smoke FROM exam_invites i
           JOIN exams e ON e.id = i.exam_id JOIN orgs o ON o.id = e.org_id
          WHERE i.id = $1"
    ))
    .bind(invite_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let (purpose, window) = decode(&row)?;
        Ok((purpose, window, row.try_get("is_smoke")?))
    })
    .transpose()
}

/// 응시 가능 시간을 바꾼다(채용 시험만 — 사내 진단이면 바꾸지 않고 거짓).
pub async fn set_window(
    pool: &PgPool,
    exam_id: Uuid,
    window: &ExamWindow,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE exams SET window_open = $2,
                window_starts_at = to_timestamp($3), window_ends_at = to_timestamp($4)
          WHERE id = $1 AND purpose = 'recruiting'",
    )
    .bind(exam_id)
    .bind(window.open)
    .bind(window.starts_at.map(|s| s as f64))
    .bind(window.ends_at.map(|s| s as f64))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}
