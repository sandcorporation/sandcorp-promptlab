//! 조직(테넌트)·구성원·동료 초대 API. 권한은 [`Role::allows`] 표로 강제한다.
//!
//! 구성원이 아닌 조직은 `404`(있는지 알리지 않는다), 구성원인데 역할이 모자라면 `403`.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use promptlab_shared::account::{Permission, Role};
use promptlab_shared::api::{
    CreateInviteRequest, CreateOrgRequest, InviteCreated, MeView, MemberView, OrgView,
    SubmissionView, UpdateMemberRequest,
};
use promptlab_shared::terms::{AcceptInviteRequest, TERMS_VERSION, TermsAcceptance};
use promptlab_store::accounts::{self, InviteOutcome, MemberChange, OrgRole};
use promptlab_store::submissions;
use uuid::Uuid;

use crate::auth::token::AccountKind;
use crate::auth::{Account, random_secret, secret_hash};
use crate::{ApiError, AppState};

/// 조직 이름 최대 길이(문자).
pub const MAX_ORG_NAME_CHARS: usize = 100;
/// 동료 초대 유효 기간(7일).
pub const INVITE_SECS: i64 = 7 * 24 * 60 * 60;
/// 조직 제출 목록 한 번에 최대.
pub const SUBMISSION_LIST_LIMIT: i64 = 200;

fn org_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "그런 조직이 없다")
}

fn view(org: OrgRole) -> OrgView {
    OrgView {
        id: org.org_id.to_string(),
        name: org.name,
        my_role: org.role,
        approved: org.approved,
        terms: org.terms.map(|(version, accepted_at)| TermsAcceptance {
            version,
            accepted_at,
        }),
    }
}

/// 동의한 약관 판이 지금 것이어야 한다(#86). 아니면 422.
fn require_terms(version: &str) -> Result<(), ApiError> {
    if version == TERMS_VERSION {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        format!("이용약관({TERMS_VERSION})에 동의해야 한다"),
    ))
}

/// 그 조직의 구성원이면 역할. 아니면 404.
pub async fn membership(
    state: &AppState,
    org_id: &str,
    account: &Account,
) -> Result<OrgRole, ApiError> {
    let org_id: Uuid = org_id.parse().map_err(|_| org_not_found())?;
    accounts::org_role(&state.pool, org_id, &account.sub)
        .await?
        .ok_or_else(org_not_found)
}

/// 구성원이고 `permission`이 있으면 역할. 구성원 아니면 404, 권한 없으면 403.
pub async fn require(
    state: &AppState,
    org_id: &str,
    account: &Account,
    permission: Permission,
) -> Result<OrgRole, ApiError> {
    let org = membership(state, org_id, account).await?;
    if !org.role.allows(permission) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "이 역할로는 할 수 없다",
        ));
    }
    Ok(org)
}

/// 내가 구성원인 조직과 역할(HTTP `/me`·MCP `list_orgs`).
pub async fn my_orgs(state: &AppState, account: &Account) -> Result<Vec<OrgView>, ApiError> {
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    let orgs = accounts::orgs_of(&state.pool, &account.sub).await?;
    Ok(orgs.into_iter().map(view).collect())
}

pub async fn me(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
) -> Result<Json<MeView>, ApiError> {
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    let orgs = accounts::orgs_of(&state.pool, &account.sub).await?;
    Ok(Json(MeView {
        name: accounts::user_name(&state.pool, &account.sub).await?,
        sub: account.sub.clone(),
        orgs: orgs.into_iter().map(view).collect(),
        is_owner: account.kind == AccountKind::Owner,
    }))
}

pub async fn create_org(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Json(request): Json<CreateOrgRequest>,
) -> Result<(StatusCode, Json<OrgView>), ApiError> {
    require_terms(&request.terms_version)?;
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > MAX_ORG_NAME_CHARS {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("조직 이름은 1~{MAX_ORG_NAME_CHARS}자다"),
        ));
    }
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    let org_id = accounts::create_org(&state.pool, &account.sub, name, TERMS_VERSION).await?;
    tracing::info!(%org_id, sub = %account.sub, "조직 만듦");
    Ok((
        StatusCode::CREATED,
        Json(OrgView {
            id: org_id.to_string(),
            name: name.to_owned(),
            my_role: Role::Admin,
            // 새 조직은 운영자 승인 전이다(#19).
            approved: false,
            terms: accounts::org_role(&state.pool, org_id, &account.sub)
                .await?
                .and_then(|org| view(org).terms),
        }),
    ))
}

pub async fn get_org(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
) -> Result<Json<OrgView>, ApiError> {
    Ok(Json(view(membership(&state, &org_id, &account).await?)))
}

pub async fn list_members(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<MemberView>>, ApiError> {
    let org = membership(&state, &org_id, &account).await?;
    let members = accounts::members(&state.pool, org.org_id).await?;
    Ok(Json(
        members
            .into_iter()
            .map(|m| MemberView {
                sub: m.sub,
                name: m.name,
                role: m.role,
            })
            .collect(),
    ))
}

fn member_change(change: MemberChange) -> Result<(), ApiError> {
    match change {
        MemberChange::Done => Ok(()),
        MemberChange::NotFound => Err(ApiError::new(StatusCode::NOT_FOUND, "그런 구성원이 없다")),
        MemberChange::LastAdmin => Err(ApiError::new(
            StatusCode::CONFLICT,
            "조직에 관리자가 한 명은 남아야 한다",
        )),
    }
}

pub async fn update_member(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, sub)): Path<(String, String)>,
    Json(request): Json<UpdateMemberRequest>,
) -> Result<Json<MemberView>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageMembers).await?;
    member_change(accounts::set_role(&state.pool, org.org_id, &sub, request.role).await?)?;
    tracing::info!(org_id = %org.org_id, %sub, role = request.role.as_str(), by = %account.sub, "역할 바꿈");
    Ok(Json(MemberView {
        name: accounts::user_name(&state.pool, &sub).await?,
        sub,
        role: request.role,
    }))
}

pub async fn remove_member(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path((org_id, sub)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageMembers).await?;
    member_change(accounts::remove_member(&state.pool, org.org_id, &sub).await?)?;
    tracing::info!(org_id = %org.org_id, %sub, by = %account.sub, "구성원 내보냄");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn create_invite(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
    Json(request): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<InviteCreated>), ApiError> {
    let org = require(&state, &org_id, &account, Permission::ManageMembers).await?;
    let token = random_secret();
    let expires_at = accounts::create_invite(
        &state.pool,
        org.org_id,
        request.role,
        &account.sub,
        &secret_hash(&token),
        INVITE_SECS,
    )
    .await?;
    tracing::info!(org_id = %org.org_id, role = request.role.as_str(), by = %account.sub, "동료 초대 만듦");
    Ok((
        StatusCode::CREATED,
        Json(InviteCreated {
            token,
            role: request.role,
            expires_at,
        }),
    ))
}

pub async fn accept_invite(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(token): Path<String>,
    Json(request): Json<AcceptInviteRequest>,
) -> Result<Json<OrgView>, ApiError> {
    require_terms(&request.terms_version)?;
    accounts::upsert_user(&state.pool, &account.sub, None).await?;
    match accounts::accept_invite(
        &state.pool,
        &secret_hash(&token),
        &account.sub,
        TERMS_VERSION,
    )
    .await?
    {
        InviteOutcome::Joined(org) => {
            tracing::info!(org_id = %org.org_id, sub = %account.sub, "초대로 조직에 들어옴");
            Ok(Json(view(org)))
        }
        InviteOutcome::AlreadyMember(org) => Ok(Json(view(org))),
        InviteOutcome::Invalid => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "초대가 없거나 만료됐거나 이미 쓰였다",
        )),
    }
}

pub async fn list_submissions(
    State(state): State<AppState>,
    Extension(account): Extension<Account>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<SubmissionView>>, ApiError> {
    let org = require(&state, &org_id, &account, Permission::ViewResults).await?;
    Ok(Json(
        submissions::list_for_org(&state.pool, org.org_id, SUBMISSION_LIST_LIMIT).await?,
    ))
}
