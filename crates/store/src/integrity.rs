//! 잠금·무결성 저장(#12). 규칙은 [`promptlab_shared::integrity`].
//!
//! - 허용 목록 거부 기록: 같은 신고값은 한 줄에 횟수를 센다. 문항마다 신고값 [`MAX_REJECTION_ROWS`]가지까지만.
//!
//! 시각은 부르는 쪽 시계(유닉스 초)로 받는다.

use promptlab_shared::integrity::{ClientIntegrity, IntegrityFailure, clip_reported};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// 문항 하나에 남기는 서로 다른 거부 신고값의 상한(거짓 신고로 줄을 끝없이 늘리지 못하게).
pub const MAX_REJECTION_ROWS: i64 = 20;

fn to_i32(n: u32) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn to_u32(n: i32) -> u32 {
    u32::try_from(n).unwrap_or(0)
}

/// 허용 목록에 걸린 문항 시작을 남긴다. 신고가 없으면(낡은 앱) 앱 판을 `(신고 없음)`으로.
pub async fn record_rejection(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
    report: Option<&ClientIntegrity>,
    failure: IntegrityFailure,
    now: i64,
) -> Result<(), sqlx::Error> {
    let (app_version, model_sha256) = match report {
        Some(r) => (
            clip_reported(&r.app_version),
            clip_reported(&r.model_sha256),
        ),
        None => ("(신고 없음)".to_owned(), String::new()),
    };
    sqlx::query(
        "INSERT INTO exam_integrity_rejections
             (invite_id, position, app_version, model_sha256, app_ok, model_ok, count, first_at, last_at)
         SELECT $1, $2, $3, $4, $5, $6, 1, to_timestamp($7), to_timestamp($7)
          WHERE (SELECT count(*) FROM exam_integrity_rejections WHERE invite_id = $1 AND position = $2) < $8
         ON CONFLICT (invite_id, position, app_version, model_sha256) DO UPDATE
            SET count = exam_integrity_rejections.count + 1, last_at = to_timestamp($7)",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .bind(app_version)
    .bind(model_sha256)
    .bind(failure.app_ok)
    .bind(failure.model_ok)
    .bind(now as f64)
    .bind(MAX_REJECTION_ROWS)
    .execute(pool)
    .await?;
    Ok(())
}

/// 거부 기록 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub position: u32,
    pub app_version: String,
    pub model_sha256: String,
    pub app_ok: bool,
    pub model_ok: bool,
    pub count: u32,
    /// 마지막 시도(유닉스 초).
    pub last_at: i64,
}

/// 초대 하나의 거부 기록(마지막 시도 순).
pub async fn rejections(pool: &PgPool, invite_id: Uuid) -> Result<Vec<Rejection>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT position, app_version, model_sha256, app_ok, model_ok, count,
                extract(epoch FROM last_at)::bigint AS last_at
           FROM exam_integrity_rejections WHERE invite_id = $1 ORDER BY last_at",
    )
    .bind(invite_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(Rejection {
                position: to_u32(r.try_get("position")?),
                app_version: r.try_get("app_version")?,
                model_sha256: r.try_get("model_sha256")?,
                app_ok: r.try_get("app_ok")?,
                model_ok: r.try_get("model_ok")?,
                count: to_u32(r.try_get("count")?),
                last_at: r.try_get("last_at")?,
            })
        })
        .collect()
}
