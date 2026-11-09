//! 과정 로그 저장(#9). 형식·검증 규칙은 [`promptlab_shared::log`]에 있고, 여기는 문항마다 순서대로
//! 검증해 붙이고(한 트랜잭션, 문항 행을 잠가 동시 업로드를 줄 세운다) 읽어 준다.
//! 시각은 부르는 쪽 시계(유닉스 초)로 받는다.

use promptlab_shared::log::{
    self, Attempt, Chain, LogError, LogEvent, MAX_EVENTS_PER_ITEM, MAX_KEYS_PER_ITEM, SignedEvent,
};

use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

fn to_i32(n: u32) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn to_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn attempt(invite_id: Uuid, position: u32) -> Attempt {
    Attempt {
        assignment_id: invite_id.to_string(),
        position,
    }
}

/// 키 등록 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// 등록했다(이미 있었어도). 서버가 아는 체인 머리.
    Registered(Chain),
    /// 이 문항에 키가 이미 [`MAX_KEYS_PER_ITEM`]개 있다.
    TooManyKeys,
}

/// 업로드 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendOutcome {
    /// 받았다(이미 받은 순번은 건너뛰었다). 새 머리.
    Accepted(Chain),
    /// 검증에 실패해 아무것도 저장하지 않았다.
    Rejected(LogError),
    /// 문항당 사건 수 한도를 넘는다.
    TooMany,
}

/// 문항 행을 잠근다(시작하지 않은 문항이면 `false`).
async fn lock_item(
    tx: &mut Transaction<'_, Postgres>,
    invite_id: Uuid,
    position: u32,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query(
        "SELECT 1 FROM exam_item_starts WHERE invite_id = $1 AND position = $2 FOR UPDATE",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_optional(&mut **tx)
    .await?
    .is_some())
}

async fn head<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    invite_id: Uuid,
    position: u32,
) -> Result<Chain, sqlx::Error> {
    let row = sqlx::query(
        "SELECT seq, hash FROM exam_log_events WHERE invite_id = $1 AND position = $2
          ORDER BY seq DESC LIMIT 1",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_optional(executor)
    .await?;
    Ok(match row {
        Some(row) => Chain {
            next_seq: u64::try_from(row.try_get::<i64, _>("seq")?).unwrap_or(0) + 1,
            head: row.try_get("hash")?,
        },
        None => Chain::default(),
    })
}

/// 이 문항에 등록된 키(등록 순).
pub async fn keys<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    invite_id: Uuid,
    position: u32,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT public_key FROM exam_log_keys WHERE invite_id = $1 AND position = $2
          ORDER BY ord",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_all(executor)
    .await
}

/// 시작한 문항에 설치 공개키를 등록한다. 시작하지 않은 문항이면 `None`. 키 모양은 부르는 쪽이 본다.
pub async fn register_key(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
    public_key: &str,
    now: i64,
) -> Result<Option<KeyOutcome>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !lock_item(&mut tx, invite_id, position).await? {
        return Ok(None);
    }
    let known = keys(&mut *tx, invite_id, position).await?;
    if !known.iter().any(|k| k == public_key) {
        if known.len() >= MAX_KEYS_PER_ITEM {
            return Ok(Some(KeyOutcome::TooManyKeys));
        }
        sqlx::query(
            "INSERT INTO exam_log_keys (invite_id, position, public_key, registered_at)
             VALUES ($1, $2, $3, to_timestamp($4))",
        )
        .bind(invite_id)
        .bind(to_i32(position))
        .bind(public_key)
        .bind(now as f64)
        .execute(&mut *tx)
        .await?;
    }
    let chain = head(&mut *tx, invite_id, position).await?;
    tx.commit().await?;
    Ok(Some(KeyOutcome::Registered(chain)))
}

/// 서명된 사건들을 순서대로 검증해 붙인다. 하나라도 맞지 않으면 아무것도 저장하지 않는다.
/// 이미 받은 순번이 같은 원문이면 건너뛴다(멱등). 시작하지 않은 문항이면 `None`.
pub async fn append(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
    events: &[SignedEvent],
    now: i64,
) -> Result<Option<AppendOutcome>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !lock_item(&mut tx, invite_id, position).await? {
        return Ok(None);
    }
    let keys = keys(&mut *tx, invite_id, position).await?;
    let mut chain = head(&mut *tx, invite_id, position).await?;
    let attempt = attempt(invite_id, position);
    let mut fresh = Vec::new();
    for event in events {
        let opened = match log::open(event, &attempt, &keys) {
            Ok(opened) => opened,
            Err(e) => return Ok(Some(AppendOutcome::Rejected(e))),
        };
        let seq = opened.body.seq;
        if seq < chain.next_seq && fresh.is_empty() {
            // 이미 받은 순번: 같은 원문이면 다시 보낸 것이다.
            let stored: Option<String> = sqlx::query_scalar(
                "SELECT hash FROM exam_log_events WHERE invite_id = $1 AND position = $2 AND seq = $3",
            )
            .bind(invite_id)
            .bind(to_i32(position))
            .bind(to_i64(seq))
            .fetch_optional(&mut *tx)
            .await?;
            if stored.as_deref() == Some(opened.hash.as_str()) {
                continue;
            }
            return Ok(Some(AppendOutcome::Rejected(LogError::Conflict { seq })));
        }
        if let Err(e) = chain.append(&opened) {
            return Ok(Some(AppendOutcome::Rejected(e)));
        }
        fresh.push((seq, event, opened.hash, opened.body.event));
    }
    if chain.next_seq > MAX_EVENTS_PER_ITEM {
        return Ok(Some(AppendOutcome::TooMany));
    }
    for (seq, event, hash, _) in fresh {
        sqlx::query(
            "INSERT INTO exam_log_events (invite_id, position, seq, body, sig, hash, received_at)
             VALUES ($1, $2, $3, $4, $5, $6, to_timestamp($7))",
        )
        .bind(invite_id)
        .bind(to_i32(position))
        .bind(to_i64(seq))
        .bind(&event.body)
        .bind(&event.sig)
        .bind(hash)
        .bind(now as f64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Some(AppendOutcome::Accepted(chain)))
}

/// 거부한 업로드를 센다(시작한 문항만).
pub async fn record_rejection(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
    reason: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO exam_log_rejections (invite_id, position, count, last_reason, last_at)
         SELECT $1, $2, 1, $3, to_timestamp($4)
          WHERE EXISTS (SELECT 1 FROM exam_item_starts WHERE invite_id = $1 AND position = $2)
         ON CONFLICT (invite_id, position) DO UPDATE
            SET count = exam_log_rejections.count + 1, last_reason = $3, last_at = to_timestamp($4)",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .bind(reason)
    .bind(now as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// 저장된 사건 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub event: SignedEvent,
    pub seq: u64,
    pub received_at: i64,
}

/// 거부 기록.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejections {
    pub count: u32,
    pub last_reason: String,
    pub last_at: i64,
}

/// 다시보기에 쓰는 문항 하나의 로그 전부.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemLog {
    pub keys: Vec<String>,
    pub events: Vec<StoredEvent>,
    pub rejections: Option<Rejections>,
}

pub async fn item_log(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
) -> Result<ItemLog, sqlx::Error> {
    let keys = keys(pool, invite_id, position).await?;
    let events = sqlx::query(
        "SELECT seq, body, sig, EXTRACT(EPOCH FROM received_at)::bigint AS received_at
           FROM exam_log_events WHERE invite_id = $1 AND position = $2 ORDER BY seq",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_all(pool)
    .await?
    .iter()
    .map(|row| {
        Ok(StoredEvent {
            event: SignedEvent {
                body: row.try_get("body")?,
                sig: row.try_get("sig")?,
            },
            seq: u64::try_from(row.try_get::<i64, _>("seq")?).unwrap_or(0),
            received_at: row.try_get("received_at")?,
        })
    })
    .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let rejections = sqlx::query(
        "SELECT count, last_reason, EXTRACT(EPOCH FROM last_at)::bigint AS last_at
           FROM exam_log_rejections WHERE invite_id = $1 AND position = $2",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_optional(pool)
    .await?
    .map(|row| {
        Ok::<_, sqlx::Error>(Rejections {
            count: u32::try_from(row.try_get::<i32, _>("count")?).unwrap_or(0),
            last_reason: row.try_get("last_reason")?,
            last_at: row.try_get("last_at")?,
        })
    })
    .transpose()?;
    Ok(ItemLog {
        keys,
        events,
        rejections,
    })
}

/// 문항의 시작 시각(유닉스 초)과 제출 id.
pub async fn item_progress(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
) -> Result<(Option<i64>, Option<Uuid>), sqlx::Error> {
    let started: Option<i64> = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM started_at)::bigint FROM exam_item_starts
          WHERE invite_id = $1 AND position = $2",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_optional(pool)
    .await?;
    let submission: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM submissions WHERE invite_id = $1 AND item_position = $2",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_optional(pool)
    .await?;
    Ok((started, submission))
}

/// 문항의 다음 순번(받은 사건 수). 시작하지 않은 문항이면 0.
pub async fn next_seq(pool: &PgPool, invite_id: Uuid, position: u32) -> Result<u64, sqlx::Error> {
    let n: Option<i64> = sqlx::query_scalar(
        "SELECT max(seq) + 1 FROM exam_log_events WHERE invite_id = $1 AND position = $2",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_one(pool)
    .await?;
    Ok(n.and_then(|n| u64::try_from(n).ok()).unwrap_or(0))
}

/// 문항의 저장된 사건(순번순, 이미 서명·체인을 검증해 받은 것)을 읽는다. 원문을 읽지 못하는 사건은 건너뛴다.
pub async fn item_events(
    pool: &PgPool,
    invite_id: Uuid,
    position: u32,
) -> Result<Vec<(u64, LogEvent)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT seq, body FROM exam_log_events WHERE invite_id = $1 AND position = $2 ORDER BY seq",
    )
    .bind(invite_id)
    .bind(to_i32(position))
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let seq = u64::try_from(row.try_get::<i64, _>("seq")?).unwrap_or(0);
        let body: String = row.try_get("body")?;
        if let Ok(body) = serde_json::from_str::<promptlab_shared::log::EventBody>(&body) {
            out.push((seq, body.event));
        }
    }
    Ok(out)
}
