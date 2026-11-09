//! 계정·조직·역할·초대, 그리고 대시보드 로그인 상태와 세션.
//!
//! 권한 판정(어느 역할이 무엇을 하나)은 [`promptlab_shared::account::Role`]에 있고, 여기는 저장만 한다.
//! 비밀(초대 토큰·세션 id·로그인 id)은 sha256 해시로만 둔다.

use promptlab_shared::account::Role;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// 해시 열에 넣을 값.
pub type SecretHash = [u8; 32];

fn decode_role(s: &str) -> Result<Role, sqlx::Error> {
    Role::parse(s).ok_or_else(|| sqlx::Error::Decode(format!("알 수 없는 역할 {s}").into()))
}

/// 사람을 기록한다(처음이면 넣고, 이름이 오면 고친다).
pub async fn upsert_user(pool: &PgPool, sub: &str, name: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users (sub, name) VALUES ($1, $2)
         ON CONFLICT (sub) DO UPDATE SET name = COALESCE(EXCLUDED.name, users.name)",
    )
    .bind(sub)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn user_name(pool: &PgPool, sub: &str) -> Result<Option<String>, sqlx::Error> {
    Ok(
        sqlx::query_scalar::<_, Option<String>>("SELECT name FROM users WHERE sub = $1")
            .bind(sub)
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}

/// 조직 하나와 그 안에서 한 사람의 역할.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgRole {
    pub org_id: Uuid,
    pub name: String,
    pub role: Role,
    /// 운영자가 승인한 조직인가(#19).
    pub approved: bool,
    /// 그 사람이 이 조직에 대해 동의한 이용약관(판, 시각 유닉스 초) — 이전 조직·구성원은 없다(#86).
    pub terms: Option<(String, i64)>,
}

/// 조직을 만들고 만든 사람을 관리자로 넣는다.
pub async fn create_org(
    pool: &PgPool,
    creator: &str,
    name: &str,
    terms_version: &str,
) -> Result<Uuid, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO orgs (name, created_by) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(creator)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query("INSERT INTO memberships (org_id, sub, role) VALUES ($1, $2, 'admin')")
        .bind(id)
        .bind(creator)
        .execute(&mut *tx)
        .await?;
    record_terms(&mut tx, id, creator, terms_version, "create_org").await?;
    tx.commit().await?;
    Ok(id)
}

/// 이용약관 동의를 남긴다(#86). 같은 조직·사람의 기록은 하나(처음 것).
async fn record_terms(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
    sub: &str,
    version: &str,
    via: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO terms_acceptances (org_id, sub, version, via) VALUES ($1, $2, $3, $4)
         ON CONFLICT (org_id, sub) DO NOTHING",
    )
    .bind(org_id)
    .bind(sub)
    .bind(version)
    .bind(via)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn terms_of(row: &sqlx::postgres::PgRow) -> Result<Option<(String, i64)>, sqlx::Error> {
    let version: Option<String> = row.try_get("terms_version")?;
    let at: Option<i64> = row.try_get("terms_at")?;
    Ok(version.zip(at))
}

/// 그 사람이 그 조직의 구성원이면 조직과 역할. 아니면(조직이 없어도) `None`.
/// 「스모크」 조직(#28)은 누구에게도 없는 조직이다 — 대시보드·결과 조회에서 빠진다.
pub async fn org_role(
    pool: &PgPool,
    org_id: Uuid,
    sub: &str,
) -> Result<Option<OrgRole>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT o.name, m.role, o.approved_at IS NOT NULL AS approved,
                t.version AS terms_version, EXTRACT(EPOCH FROM t.accepted_at)::bigint AS terms_at
           FROM memberships m JOIN orgs o ON o.id = m.org_id
           LEFT JOIN terms_acceptances t ON t.org_id = m.org_id AND t.sub = m.sub
          WHERE m.org_id = $1 AND m.sub = $2 AND NOT o.is_smoke",
    )
    .bind(org_id)
    .bind(sub)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(OrgRole {
            org_id,
            name: row.try_get("name")?,
            role: decode_role(row.try_get("role")?)?,
            approved: row.try_get("approved")?,
            terms: terms_of(&row)?,
        })
    })
    .transpose()
}

/// 그 사람이 속한 조직들(만든 순). 「스모크」 조직은 빠진다.
pub async fn orgs_of(pool: &PgPool, sub: &str) -> Result<Vec<OrgRole>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT o.id, o.name, m.role, o.approved_at IS NOT NULL AS approved,
                t.version AS terms_version, EXTRACT(EPOCH FROM t.accepted_at)::bigint AS terms_at
           FROM memberships m JOIN orgs o ON o.id = m.org_id
           LEFT JOIN terms_acceptances t ON t.org_id = m.org_id AND t.sub = m.sub
          WHERE m.sub = $1 AND NOT o.is_smoke ORDER BY o.created_at, o.id",
    )
    .bind(sub)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OrgRole {
                org_id: row.try_get("id")?,
                name: row.try_get("name")?,
                role: decode_role(row.try_get("role")?)?,
                approved: row.try_get("approved")?,
                terms: terms_of(&row)?,
            })
        })
        .collect()
}

/// 구성원 한 명.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub sub: String,
    pub name: Option<String>,
    pub role: Role,
}

pub async fn members(pool: &PgPool, org_id: Uuid) -> Result<Vec<Member>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT m.sub, u.name, m.role FROM memberships m JOIN users u ON u.sub = m.sub
          WHERE m.org_id = $1 ORDER BY m.joined_at, m.sub",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Member {
                sub: row.try_get("sub")?,
                name: row.try_get("name")?,
                role: decode_role(row.try_get("role")?)?,
            })
        })
        .collect()
}

/// 구성원 바꾸기의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberChange {
    Done,
    /// 그런 구성원이 없다.
    NotFound,
    /// 마지막 관리자를 강등·내보내려 했다(조직에 관리자가 하나는 남아야 한다).
    LastAdmin,
}

/// 관리자 수를 세는 동안 다른 바꾸기와 엇갈리지 않게 조직 행을 잠근다.
async fn lock_org(tx: &mut sqlx::PgConnection, org_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT id FROM orgs WHERE id = $1 FOR UPDATE")
        .bind(org_id)
        .execute(tx)
        .await?;
    Ok(())
}

async fn would_remove_last_admin(
    tx: &mut sqlx::PgConnection,
    org_id: Uuid,
    sub: &str,
) -> Result<Option<bool>, sqlx::Error> {
    let current: Option<String> =
        sqlx::query_scalar("SELECT role FROM memberships WHERE org_id = $1 AND sub = $2")
            .bind(org_id)
            .bind(sub)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(current) = current else {
        return Ok(None);
    };
    if current != "admin" {
        return Ok(Some(false));
    }
    let admins: i64 =
        sqlx::query_scalar("SELECT count(*) FROM memberships WHERE org_id = $1 AND role = 'admin'")
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await?;
    Ok(Some(admins <= 1))
}

pub async fn set_role(
    pool: &PgPool,
    org_id: Uuid,
    sub: &str,
    role: Role,
) -> Result<MemberChange, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_org(&mut tx, org_id).await?;
    match would_remove_last_admin(&mut tx, org_id, sub).await? {
        None => return Ok(MemberChange::NotFound),
        Some(true) if role != Role::Admin => return Ok(MemberChange::LastAdmin),
        Some(_) => {}
    }
    sqlx::query("UPDATE memberships SET role = $3 WHERE org_id = $1 AND sub = $2")
        .bind(org_id)
        .bind(sub)
        .bind(role.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(MemberChange::Done)
}

pub async fn remove_member(
    pool: &PgPool,
    org_id: Uuid,
    sub: &str,
) -> Result<MemberChange, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_org(&mut tx, org_id).await?;
    match would_remove_last_admin(&mut tx, org_id, sub).await? {
        None => return Ok(MemberChange::NotFound),
        Some(true) => return Ok(MemberChange::LastAdmin),
        Some(false) => {}
    }
    sqlx::query("DELETE FROM memberships WHERE org_id = $1 AND sub = $2")
        .bind(org_id)
        .bind(sub)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(MemberChange::Done)
}

/// 초대를 저장하고 만료 시각(유닉스 초)을 준다.
pub async fn create_invite(
    pool: &PgPool,
    org_id: Uuid,
    role: Role,
    creator: &str,
    token_hash: &SecretHash,
    valid_secs: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO org_invites (token_hash, org_id, role, created_by, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))
         RETURNING extract(epoch FROM expires_at)::bigint",
    )
    .bind(&token_hash[..])
    .bind(org_id)
    .bind(role.as_str())
    .bind(creator)
    .bind(valid_secs as f64)
    .fetch_one(pool)
    .await
}

/// 초대 받기의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteOutcome {
    Joined(OrgRole),
    /// 이미 구성원이다 — 역할은 그대로 둔다(초대는 쓰지 않은 채 남는다).
    AlreadyMember(OrgRole),
    /// 없거나 만료됐거나 이미 썼다.
    Invalid,
}

/// 초대를 받아 구성원이 된다. 초대는 한 번만 쓸 수 있다.
pub async fn accept_invite(
    pool: &PgPool,
    token_hash: &SecretHash,
    sub: &str,
    terms_version: &str,
) -> Result<InviteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT org_id, role FROM org_invites
          WHERE token_hash = $1 AND accepted_by IS NULL AND expires_at > now()
          FOR UPDATE",
    )
    .bind(&token_hash[..])
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(InviteOutcome::Invalid);
    };
    let org_id: Uuid = row.try_get("org_id")?;
    let role = decode_role(row.try_get("role")?)?;
    let existing: Option<String> =
        sqlx::query_scalar("SELECT role FROM memberships WHERE org_id = $1 AND sub = $2")
            .bind(org_id)
            .bind(sub)
            .fetch_optional(&mut *tx)
            .await?;
    let org =
        sqlx::query("SELECT name, approved_at IS NOT NULL AS approved FROM orgs WHERE id = $1")
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await?;
    let name: String = org.try_get("name")?;
    let approved: bool = org.try_get("approved")?;
    if let Some(existing) = existing {
        return Ok(InviteOutcome::AlreadyMember(OrgRole {
            org_id,
            name,
            role: decode_role(&existing)?,
            approved,
            terms: existing_terms(&mut tx, org_id, sub).await?,
        }));
    }
    sqlx::query("INSERT INTO memberships (org_id, sub, role) VALUES ($1, $2, $3)")
        .bind(org_id)
        .bind(sub)
        .bind(role.as_str())
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE org_invites SET accepted_by = $2, accepted_at = now() WHERE token_hash = $1",
    )
    .bind(&token_hash[..])
    .bind(sub)
    .execute(&mut *tx)
    .await?;
    record_terms(&mut tx, org_id, sub, terms_version, "invite").await?;
    let terms = existing_terms(&mut tx, org_id, sub).await?;
    tx.commit().await?;
    Ok(InviteOutcome::Joined(OrgRole {
        org_id,
        name,
        role,
        approved,
        terms,
    }))
}

async fn existing_terms(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
    sub: &str,
) -> Result<Option<(String, i64)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT version AS terms_version, EXTRACT(EPOCH FROM accepted_at)::bigint AS terms_at
           FROM terms_acceptances WHERE org_id = $1 AND sub = $2",
    )
    .bind(org_id)
    .bind(sub)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| terms_of(&row))
        .transpose()
        .map(Option::flatten)
}

/// 대시보드 로그인 진행 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginState {
    pub state: String,
    pub verifier: String,
    pub next_path: String,
}

pub async fn save_login_state(
    pool: &PgPool,
    id_hash: &SecretHash,
    login: &LoginState,
    valid_secs: i64,
) -> Result<(), sqlx::Error> {
    // 오래된 행은 여기서 함께 치운다(로그인은 드물다).
    sqlx::query("DELETE FROM login_states WHERE expires_at < now()")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO login_states (id_hash, state, verifier, next_path, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
    )
    .bind(&id_hash[..])
    .bind(&login.state)
    .bind(&login.verifier)
    .bind(&login.next_path)
    .bind(valid_secs as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// 로그인 상태를 꺼내며 지운다(한 번만 쓴다). 만료됐으면 `None`.
pub async fn take_login_state(
    pool: &PgPool,
    id_hash: &SecretHash,
) -> Result<Option<LoginState>, sqlx::Error> {
    let row = sqlx::query(
        "DELETE FROM login_states WHERE id_hash = $1
         RETURNING state, verifier, next_path, expires_at > now() AS live",
    )
    .bind(&id_hash[..])
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    if !row.try_get::<bool, _>("live")? {
        return Ok(None);
    }
    Ok(Some(LoginState {
        state: row.try_get("state")?,
        verifier: row.try_get("verifier")?,
        next_path: row.try_get("next_path")?,
    }))
}

/// 세션의 주인.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOwner {
    pub sub: String,
    pub is_owner: bool,
}

pub async fn create_session(
    pool: &PgPool,
    id_hash: &SecretHash,
    owner: &SessionOwner,
    valid_secs: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE expires_at < now()")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO sessions (id_hash, sub, is_owner, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(secs => $4))",
    )
    .bind(&id_hash[..])
    .bind(&owner.sub)
    .bind(owner.is_owner)
    .bind(valid_secs as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// 살아 있는 세션의 주인.
pub async fn session(
    pool: &PgPool,
    id_hash: &SecretHash,
) -> Result<Option<SessionOwner>, sqlx::Error> {
    let row =
        sqlx::query("SELECT sub, is_owner FROM sessions WHERE id_hash = $1 AND expires_at > now()")
            .bind(&id_hash[..])
            .fetch_optional(pool)
            .await?;
    row.map(|row| {
        Ok(SessionOwner {
            sub: row.try_get("sub")?,
            is_owner: row.try_get("is_owner")?,
        })
    })
    .transpose()
}

pub async fn delete_session(pool: &PgPool, id_hash: &SecretHash) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id_hash = $1")
        .bind(&id_hash[..])
        .execute(pool)
        .await?;
    Ok(())
}
