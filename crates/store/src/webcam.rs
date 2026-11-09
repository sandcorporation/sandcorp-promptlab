//! 웹캠 동의·사진 저장(#13). 규칙은 [`promptlab_shared::webcam`].
//!
//! 사진은 Postgres(`exam_webcam_photos.jpeg`, 장당 64KB 이하)에 둔다 — 운영 인프라(볼륨·객체 저장소)를 늘리지 않는다.
//! 보유 기간이 지난 사진은 [`purge_older_than`]으로 지운다(서버가 주기적으로 부른다).
//! 시각은 부르는 쪽 시계(유닉스 초)로 받는다.

use promptlab_shared::webcam::{BaselineMeta, SnapshotMeta, SnapshotReason};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// 동의를 남긴다(같은 배정은 덮어쓴다).
pub async fn record_consent(
    pool: &PgPool,
    invite_id: Uuid,
    notice_version: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO exam_webcam_consents (invite_id, notice_version, consented_at)
         VALUES ($1, $2, to_timestamp($3))
         ON CONFLICT (invite_id) DO UPDATE
            SET notice_version = EXCLUDED.notice_version, consented_at = EXCLUDED.consented_at",
    )
    .bind(invite_id)
    .bind(notice_version)
    .bind(now as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// 마지막 동의(안내문 판, 유닉스 초).
pub async fn consent(pool: &PgPool, invite_id: Uuid) -> Result<Option<(String, i64)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT notice_version, EXTRACT(EPOCH FROM consented_at)::bigint AS at
           FROM exam_webcam_consents WHERE invite_id = $1",
    )
    .bind(invite_id)
    .fetch_optional(pool)
    .await?;
    row.map(|r| Ok((r.try_get("notice_version")?, r.try_get("at")?)))
        .transpose()
}

/// 기준 사진 정보.
pub async fn baseline(pool: &PgPool, invite_id: Uuid) -> Result<Option<BaselineMeta>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, (EXTRACT(EPOCH FROM taken_at) * 1000)::bigint AS at_ms
           FROM exam_webcam_photos WHERE invite_id = $1 AND reason = 'baseline'",
    )
    .bind(invite_id)
    .fetch_optional(pool)
    .await?;
    row.map(|r| {
        Ok(BaselineMeta {
            id: r.try_get::<Uuid, _>("id")?.to_string(),
            at_ms: r.try_get("at_ms")?,
        })
    })
    .transpose()
}

/// 기준 사진을 넣거나 바꾼다.
pub async fn put_baseline(
    pool: &PgPool,
    invite_id: Uuid,
    jpeg: &[u8],
    now: i64,
) -> Result<Uuid, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM exam_webcam_photos WHERE invite_id = $1 AND reason = 'baseline'")
        .bind(invite_id)
        .execute(&mut *tx)
        .await?;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO exam_webcam_photos (id, invite_id, position, reason, taken_at, jpeg)
         VALUES ($1, $2, NULL, 'baseline', to_timestamp($3), $4)",
    )
    .bind(id)
    .bind(invite_id)
    .bind(now as f64)
    .bind(jpeg)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// 새 스냅샷.
pub struct NewSnapshot<'a> {
    pub invite_id: Uuid,
    pub position: u32,
    pub reason: SnapshotReason,
    pub client_ref: Option<&'a str>,
    pub app_at_ms: Option<i64>,
    pub jpeg: &'a [u8],
}

/// 스냅샷 저장 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// 새로 넣었다.
    Stored(Uuid),
    /// 같은 `client_ref`가 이미 있다(다시 올림 — 멱등).
    Duplicate(Uuid),
    /// 문항의 스냅샷 상한을 넘는다.
    Full,
}

/// 스냅샷을 넣는다. 문항마다 `limit`장까지.
pub async fn add_snapshot(
    pool: &PgPool,
    snap: &NewSnapshot<'_>,
    limit: i64,
    now: i64,
) -> Result<SnapshotOutcome, sqlx::Error> {
    let position = i32::try_from(snap.position).unwrap_or(i32::MAX);
    if let Some(r) = snap.client_ref {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM exam_webcam_photos WHERE invite_id = $1 AND client_ref = $2",
        )
        .bind(snap.invite_id)
        .bind(r)
        .fetch_optional(pool)
        .await?;
        if let Some(id) = existing {
            return Ok(SnapshotOutcome::Duplicate(id));
        }
    }
    let id = Uuid::new_v4();
    // 상한 검사와 넣기를 한 문장으로 — 동시에 올려도 상한을 넘지 않는다(넘으면 0행).
    let inserted = sqlx::query(
        "INSERT INTO exam_webcam_photos (id, invite_id, position, reason, client_ref, app_at_ms, taken_at, jpeg)
         SELECT $1, $2, $3, $4, $5, $6, to_timestamp($7), $8
          WHERE (SELECT count(*) FROM exam_webcam_photos WHERE invite_id = $2 AND position = $3) < $9
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(snap.invite_id)
    .bind(position)
    .bind(snap.reason.as_str())
    .bind(snap.client_ref)
    .bind(snap.app_at_ms)
    .bind(now as f64)
    .bind(snap.jpeg)
    .bind(limit)
    .execute(pool)
    .await?
    .rows_affected();
    if inserted == 1 {
        return Ok(SnapshotOutcome::Stored(id));
    }
    // 같은 참조가 동시에 들어왔으면 그것을, 아니면 상한.
    if let Some(r) = snap.client_ref {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM exam_webcam_photos WHERE invite_id = $1 AND client_ref = $2",
        )
        .bind(snap.invite_id)
        .bind(r)
        .fetch_optional(pool)
        .await?;
        if let Some(id) = existing {
            return Ok(SnapshotOutcome::Duplicate(id));
        }
    }
    Ok(SnapshotOutcome::Full)
}

/// 배정의 스냅샷(기준 사진 제외) 시간순.
pub async fn snapshots(pool: &PgPool, invite_id: Uuid) -> Result<Vec<SnapshotMeta>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, position, reason, client_ref, app_at_ms,
                (EXTRACT(EPOCH FROM taken_at) * 1000)::bigint AS at_ms
           FROM exam_webcam_photos
          WHERE invite_id = $1 AND reason <> 'baseline'
          ORDER BY taken_at, app_at_ms NULLS LAST, id",
    )
    .bind(invite_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            let reason: String = r.try_get("reason")?;
            Ok(SnapshotMeta {
                id: r.try_get::<Uuid, _>("id")?.to_string(),
                position: u32::try_from(r.try_get::<i32, _>("position")?).unwrap_or(0),
                reason: SnapshotReason::parse(&reason).ok_or_else(|| {
                    sqlx::Error::Decode(format!("모르는 스냅샷 까닭: {reason}").into())
                })?,
                client_ref: r.try_get("client_ref")?,
                at_ms: r.try_get("at_ms")?,
                app_at_ms: r.try_get("app_at_ms")?,
            })
        })
        .collect()
}

/// 배정의 사진 하나(기준 사진 포함). 다른 배정의 사진은 없다.
pub async fn photo(
    pool: &PgPool,
    invite_id: Uuid,
    photo_id: Uuid,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    sqlx::query_scalar("SELECT jpeg FROM exam_webcam_photos WHERE invite_id = $1 AND id = $2")
        .bind(invite_id)
        .bind(photo_id)
        .fetch_optional(pool)
        .await
}

/// `cutoff`(유닉스 초)보다 먼저 찍은 사진을 지운다. 지운 장수.
pub async fn purge_older_than(pool: &PgPool, cutoff: i64) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM exam_webcam_photos WHERE taken_at < to_timestamp($1)")
            .bind(cutoff as f64)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}
