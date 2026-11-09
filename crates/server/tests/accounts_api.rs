//! 시험 이음매 ②: 계정·조직·역할·격리·대시보드 로그인. 라우터에 프로세스 안에서 요청을 보낸다.
//! 토큰은 시험 키로 서명하고(가짜 JWKS), auth `/token`은 가짜다. 모든 응답에 숨은 입력 불변식이 걸린다(`common::send`).

mod common;

use axum::http::{Request, StatusCode, header};
use common::*;
use promptlab_server::auth::login::TokenResponse;
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AppAuthConfig, InviteCreated, MeView, MemberView, OrgView, SubmissionView,
};
use promptlab_shared::terms::TERMS_VERSION;
use promptlab_store::PgPool;
use serde_json::{Value, json};

async fn create_org(app: &axum::Router, sub: &str, name: &str) -> OrgView {
    let (status, body) = call(
        app,
        "POST",
        "/orgs",
        Some(&token_for(sub)),
        Some(json!({"name": name, "terms_version": promptlab_shared::terms::TERMS_VERSION})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

/// `admin`이 `role`로 초대하고 `sub`가 받는다.
async fn join(app: &axum::Router, org: &OrgView, admin: &str, sub: &str, role: Role) {
    let (status, body) = call(
        app,
        "POST",
        &format!("/orgs/{}/invites", org.id),
        Some(&token_for(admin)),
        Some(json!({"role": role})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let invite: InviteCreated = serde_json::from_value(body).unwrap();
    let (status, body) = call(
        app,
        "POST",
        &format!("/invites/{}/accept", invite.token),
        Some(&token_for(sub)),
        Some(json!({"terms_version": promptlab_shared::terms::TERMS_VERSION})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_value::<OrgView>(body).unwrap().my_role,
        role
    );
}

/// ALICE(관리자)의 조직에 BOB 출제자, CAROL 감독관, DAVE 열람자.
async fn staffed_org(app: &axum::Router) -> OrgView {
    let org = create_org(app, ALICE, "에이 회사").await;
    join(app, &org, ALICE, BOB, Role::Author).await;
    join(app, &org, ALICE, CAROL, Role::Proctor).await;
    join(app, &org, ALICE, DAVE, Role::Viewer).await;
    org
}

async fn submit(app: &axum::Router, sub: &str, org_id: Option<&str>) -> (StatusCode, Value) {
    let body =
        serde_json::to_value(common::practice_request("array-sum", "correct", 60, org_id)).unwrap();
    call(
        app,
        "POST",
        "/submissions",
        Some(&token_for(sub)),
        Some(body),
    )
    .await
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 조직을_만들면_만든_사람이_관리자이고_me에_보인다(pool: PgPool) {
    let app = app(pool);
    let org = create_org(&app, ALICE, "  에이 회사 ").await;
    assert_eq!(org.name, "에이 회사");
    assert_eq!(org.my_role, Role::Admin);

    let (status, body) = call(&app, "GET", "/me", Some(&token_for(ALICE)), None).await;
    assert_eq!(status, StatusCode::OK);
    let me: MeView = serde_json::from_value(body).unwrap();
    assert_eq!(me.sub, ALICE);
    assert_eq!(me.orgs, vec![org]);

    let (_, body) = call(&app, "GET", "/me", Some(&token_for(BOB)), None).await;
    assert!(
        serde_json::from_value::<MeView>(body)
            .unwrap()
            .orgs
            .is_empty()
    );

    for name in ["", "   ", &"가".repeat(101)] {
        let (status, _) = call(
            &app,
            "POST",
            "/orgs",
            Some(&token_for(ALICE)),
            Some(json!({"name": name, "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 역할별_허용과_거부(pool: PgPool) {
    let app = app(pool);
    let org = staffed_org(&app).await;
    let members = format!("/orgs/{}/members", org.id);
    let invites = format!("/orgs/{}/invites", org.id);
    let results = format!("/orgs/{}/submissions", org.id);

    // 모든 구성원: 조직·구성원 보기.
    for sub in [ALICE, BOB, CAROL, DAVE] {
        let (status, _) = call(
            &app,
            "GET",
            &format!("/orgs/{}", org.id),
            Some(&token_for(sub)),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{sub}");
        let (status, body) = call(&app, "GET", &members, Some(&token_for(sub)), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_value::<Vec<MemberView>>(body)
                .unwrap()
                .len(),
            4
        );
    }

    // 구성원 관리(초대·역할 바꾸기·내보내기): 관리자만.
    for sub in [BOB, CAROL, DAVE] {
        let token = token_for(sub);
        let (status, _) = call(
            &app,
            "POST",
            &invites,
            Some(&token),
            Some(json!({"role": "viewer"})),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{sub} 초대");
        let (status, _) = call(
            &app,
            "PATCH",
            &format!("{members}/{DAVE}"),
            Some(&token),
            Some(json!({"role": "admin"})),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{sub} 역할");
        let (status, _) = call(
            &app,
            "DELETE",
            &format!("{members}/{DAVE}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{sub} 내보내기");
    }

    // 응시 결과 보기: 관리자·감독관·열람자. 출제자는 못 본다.
    for (sub, expected) in [
        (ALICE, StatusCode::OK),
        (CAROL, StatusCode::OK),
        (DAVE, StatusCode::OK),
        (BOB, StatusCode::FORBIDDEN),
    ] {
        let (status, _) = call(&app, "GET", &results, Some(&token_for(sub)), None).await;
        assert_eq!(status, expected, "{sub} 결과");
    }

    // 관리자는 역할을 바꾸고 내보낸다.
    let (status, body) = call(
        &app,
        "PATCH",
        &format!("{members}/{DAVE}"),
        Some(&token_for(ALICE)),
        Some(json!({"role": "proctor"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_value::<MemberView>(body).unwrap().role,
        Role::Proctor
    );
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("{members}/{DAVE}"),
        Some(&token_for(ALICE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &app,
        "GET",
        &format!("/orgs/{}", org.id),
        Some(&token_for(DAVE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "내보낸 사람은 더 못 본다");
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 마지막_관리자는_강등하거나_내보낼_수_없다(pool: PgPool) {
    let app = app(pool);
    let org = create_org(&app, ALICE, "에이").await;
    let me = format!("/orgs/{}/members/{ALICE}", org.id);
    let (status, _) = call(
        &app,
        "PATCH",
        &me,
        Some(&token_for(ALICE)),
        Some(json!({"role": "viewer"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(&app, "DELETE", &me, Some(&token_for(ALICE)), None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // 관리자가 둘이면 한 명은 내려갈 수 있다.
    join(&app, &org, ALICE, BOB, Role::Admin).await;
    let (status, _) = call(
        &app,
        "PATCH",
        &me,
        Some(&token_for(ALICE)),
        Some(json!({"role": "viewer"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/orgs/{}/members/{ERIN}", org.id),
        Some(&token_for(BOB)),
        Some(json!({"role": "viewer"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "없는 구성원");
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 초대는_한_번만_쓰이고_틀린_초대는_404(pool: PgPool) {
    let app = app(pool);
    let org = create_org(&app, ALICE, "에이").await;
    let (_, body) = call(
        &app,
        "POST",
        &format!("/orgs/{}/invites", org.id),
        Some(&token_for(ALICE)),
        Some(json!({"role": "viewer"})),
    )
    .await;
    let invite: InviteCreated = serde_json::from_value(body).unwrap();
    assert_eq!(invite.role, Role::Viewer);

    let accept = format!("/invites/{}/accept", invite.token);
    let agree = Some(json!({"terms_version": TERMS_VERSION}));
    let (status, _) = call(&app, "POST", &accept, Some(&token_for(BOB)), agree.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "POST",
        &accept,
        Some(&token_for(CAROL)),
        agree.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "이미 쓴 초대");
    let (status, _) = call(
        &app,
        "POST",
        "/invites/없는-초대/accept",
        Some(&token_for(CAROL)),
        agree,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 이미 구성원이면 역할을 바꾸지 않는다(관리자가 열람자 초대를 받아도 관리자).
    let (_, body) = call(
        &app,
        "POST",
        &format!("/orgs/{}/invites", org.id),
        Some(&token_for(ALICE)),
        Some(json!({"role": "viewer"})),
    )
    .await;
    let invite: InviteCreated = serde_json::from_value(body).unwrap();
    let (status, body) = call(
        &app,
        "POST",
        &format!("/invites/{}/accept", invite.token),
        Some(&token_for(ALICE)),
        Some(json!({"terms_version": promptlab_shared::terms::TERMS_VERSION})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_value::<OrgView>(body).unwrap().my_role,
        Role::Admin
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 다른_조직의_데이터에는_닿지_못한다(pool: PgPool) {
    let app = app(pool);
    let a = staffed_org(&app).await; // ALICE 관리자, DAVE 열람자
    let b = create_org(&app, ERIN, "비 회사").await;

    // 조직 응시 제출: A의 감독관 CAROL이 A로, B의 ERIN이 B로.
    let (status, body) = submit(&app, CAROL, Some(&a.id)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let in_a: SubmissionView = serde_json::from_value(body).unwrap();
    assert_eq!(in_a.org_id.as_deref(), Some(a.id.as_str()));
    let (_, body) = submit(&app, ERIN, Some(&b.id)).await;
    let in_b: SubmissionView = serde_json::from_value(body).unwrap();

    // B 관리자는 A의 조직·구성원·결과·초대·역할에 닿지 못한다 — 있는지도 모른다(404).
    let erin = token_for(ERIN);
    for (method, uri, body) in [
        ("GET", format!("/orgs/{}", a.id), None),
        ("GET", format!("/orgs/{}/members", a.id), None),
        ("GET", format!("/orgs/{}/submissions", a.id), None),
        (
            "POST",
            format!("/orgs/{}/invites", a.id),
            Some(json!({"role": "admin"})),
        ),
        (
            "PATCH",
            format!("/orgs/{}/members/{ALICE}", a.id),
            Some(json!({"role": "viewer"})),
        ),
        ("DELETE", format!("/orgs/{}/members/{ALICE}", a.id), None),
        ("GET", format!("/submissions/{}", in_a.id), None),
    ] {
        let (status, _) = call(&app, method, &uri, Some(&erin), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    // B의 사람은 A 조직 이름으로 제출하지 못한다.
    let (status, _) = submit(&app, ERIN, Some(&a.id)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 모양이 틀린 조직 id도 404.
    let (status, _) = call(&app, "GET", "/orgs/uuid-아님", Some(&erin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A의 결과 목록에는 A의 제출만.
    let (_, body) = call(
        &app,
        "GET",
        &format!("/orgs/{}/submissions", a.id),
        Some(&token_for(DAVE)),
        None,
    )
    .await;
    let listed: Vec<SubmissionView> = serde_json::from_value(body).unwrap();
    assert_eq!(
        listed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec![in_a.id.as_str()]
    );

    // A의 열람자는 A 제출을 보고, B 제출은 못 본다. 출제자는 결과 권한이 없어 A 제출도 못 본다.
    let (status, _) = call(
        &app,
        "GET",
        &format!("/submissions/{}", in_a.id),
        Some(&token_for(DAVE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "GET",
        &format!("/submissions/{}", in_b.id),
        Some(&token_for(DAVE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "GET",
        &format!("/submissions/{}", in_a.id),
        Some(&token_for(BOB)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 낸 사람은 자기 제출을 본다.
    let (status, _) = call(
        &app,
        "GET",
        &format!("/submissions/{}", in_a.id),
        Some(&token_for(CAROL)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 토큰 거부가 HTTP 응답(401·403과 `WWW-Authenticate`)으로 나온다(auth 문서 5절).
#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 잘못된_토큰은_401_스코프_부족은_403(pool: PgPool) {
    let app = app(pool);
    let metadata = "https://promptlab.sandcorp.co.kr/.well-known/oauth-protected-resource/api";
    let base = member_claims(ALICE);
    let mut bad = vec![
        (
            "서명",
            sign_parts(&json!({"alg": "EdDSA", "kid": KID}), &base, &key(9)),
        ),
        (
            "alg",
            sign_parts(&json!({"alg": "HS256", "kid": KID}), &base, &key(1)),
        ),
    ];
    for (what, claim, value) in [
        ("aud", "aud", json!("https://sandcorp.co.kr/mcp")),
        ("iss", "iss", json!("https://auth.sandcorp.co.kr/")),
        ("만료", "exp", json!(NOW - 31)),
        ("acct", "acct", json!("owner")),
    ] {
        let mut claims = base.clone();
        claims[claim] = value;
        bad.push((what, sign(&claims)));
    }
    for (what, token) in bad {
        let request = Request::builder()
            .uri("/me")
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let (status, headers, _) = send(&app, request, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}");
        assert_eq!(
            headers[header::WWW_AUTHENTICATE],
            format!("Bearer resource_metadata=\"{metadata}\", error=\"invalid_token\""),
            "{what}"
        );
    }

    // 토큰 없음·Bearer 아님: error 없는 401.
    for authorization in [None, Some("Basic YTpi")] {
        let mut request = Request::builder().uri("/me");
        if let Some(a) = authorization {
            request = request.header(header::AUTHORIZATION, a);
        }
        let (status, headers, _) = send(&app, request, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            headers[header::WWW_AUTHENTICATE],
            format!("Bearer resource_metadata=\"{metadata}\"")
        );
    }

    // 스코프 부족.
    let mut claims = base.clone();
    claims["scope"] = json!("login");
    let request = Request::builder()
        .uri("/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", sign(&claims)));
    let (status, headers, _) = send(&app, request, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        headers[header::WWW_AUTHENTICATE],
        "Bearer error=\"insufficient_scope\", scope=\"promptlab\""
    );

    // 대표 토큰(acct 없음)은 받는다.
    let mut owner = base.clone();
    owner.as_object_mut().unwrap().remove("acct");
    owner["sub"] = json!(OWNER_SUB);
    let (status, _) = call(&app, "GET", "/me", Some(&sign(&owner)), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn api_접두로도_같은_경로를_받는다(pool: PgPool) {
    let app = app(pool);
    let (status, _) = call(&app, "GET", "/api/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", "/api/me", Some(&token_for(ALICE)), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", "/api/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 공개_문서_메타데이터_cimd_앱_로그인_설정(pool: PgPool) {
    let app = app(pool);
    let (status, body) = call(
        &app,
        "GET",
        "/.well-known/oauth-protected-resource/api",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "resource": AUDIENCE,
            "authorization_servers": [ISSUER],
            "scopes_supported": ["promptlab"],
            "bearer_methods_supported": ["header"],
        })
    );

    let client_id = "https://promptlab.sandcorp.co.kr/api/oauth/app-client.json";
    let (status, body) = call(&app, "GET", "/api/oauth/app-client.json", None, None).await;
    assert_eq!(status, StatusCode::OK);
    // auth의 CIMD 검사: client_id = 문서 URL, client_name, 루프백 redirect_uris, 인증 none.
    assert_eq!(body["client_id"], client_id);
    assert!(body["client_name"].as_str().is_some_and(|n| !n.is_empty()));
    assert_eq!(body["redirect_uris"], json!(["http://127.0.0.1/callback"]));
    assert_eq!(body["token_endpoint_auth_method"], "none");

    let (status, body) = call(&app, "GET", "/auth/app-config", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let config: AppAuthConfig = serde_json::from_value(body).unwrap();
    assert_eq!(config.client_id, client_id);
    assert_eq!(config.resource, AUDIENCE);
    assert_eq!(
        config.authorization_endpoint,
        "https://auth.sandcorp.co.kr/authorize"
    );
    assert_eq!(config.token_endpoint, "https://auth.sandcorp.co.kr/token");
    assert_eq!(config.scope, "promptlab");
}

fn set_cookies(headers: &axum::http::HeaderMap) -> Vec<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> String {
    set_cookies(headers)
        .into_iter()
        .find_map(|c| {
            c.strip_prefix(&format!("{name}="))
                .map(|rest| rest.split(';').next().unwrap().to_owned())
        })
        .unwrap_or_else(|| panic!("{name} 쿠키 없음"))
}

fn query_param(url: &str, key: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| panic!("{key} 없음: {url}"))
}

/// 대시보드 로그인 시작 → (authorize URL, 로그인 쿠키).
async fn start_login(app: &axum::Router) -> (String, String) {
    let (status, headers, _) = send(
        app,
        Request::builder().uri("/api/auth/login?next=/orgs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers[header::LOCATION].to_str().unwrap().to_owned();
    (location, cookie_value(&headers, "pl_login"))
}

async fn callback(
    app: &axum::Router,
    login_cookie: &str,
    query: &str,
) -> (StatusCode, axum::http::HeaderMap) {
    let request = Request::builder()
        .uri(format!("/api/auth/callback?{query}"))
        .header(header::COOKIE, format!("pl_login={login_cookie}"));
    let (status, headers, _) = send(app, request, None).await;
    (status, headers)
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 대시보드_로그인은_코드를_교환해_세션_쿠키를_준다(pool: PgPool) {
    // ID 토큰(aud=client_id)에 표시 이름.
    let mut id_claims = member_claims(ALICE);
    id_claims["aud"] = json!(DASHBOARD_CLIENT_ID);
    id_claims["name"] = json!("앨리스");
    let endpoint = FakeTokenEndpoint::answering(Ok(TokenResponse {
        access_token: token_for(ALICE),
        id_token: Some(sign(&id_claims)),
    }));
    let app = app_with(pool, endpoint.clone());

    let (authorize, login_cookie) = start_login(&app).await;
    assert!(authorize.starts_with("https://auth.sandcorp.co.kr/authorize?"));
    assert_eq!(query_param(&authorize, "response_type"), "code");
    assert_eq!(query_param(&authorize, "client_id"), DASHBOARD_CLIENT_ID);
    assert_eq!(
        query_param(&authorize, "redirect_uri"),
        "https://promptlab.sandcorp.co.kr/api/auth/callback"
    );
    assert_eq!(query_param(&authorize, "code_challenge_method"), "S256");
    assert_eq!(query_param(&authorize, "resource"), AUDIENCE);
    assert_eq!(query_param(&authorize, "scope"), "openid profile promptlab");
    assert!(
        !authorize.contains("code_verifier"),
        "verifier는 URL에 싣지 않는다"
    );
    let state = query_param(&authorize, "state");
    let challenge = query_param(&authorize, "code_challenge");

    let (status, headers) = callback(&app, &login_cookie, &format!("code=c1&state={state}")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers[header::LOCATION],
        "https://promptlab.sandcorp.co.kr/orgs"
    );
    let session = cookie_value(&headers, "pl_session");
    let cookie_line = set_cookies(&headers)
        .into_iter()
        .find(|c| c.starts_with("pl_session="))
        .unwrap();
    for attr in ["HttpOnly", "Secure", "SameSite=Lax", "Max-Age=43200"] {
        assert!(cookie_line.contains(attr), "{cookie_line}");
    }

    // 교환 요청: 같은 redirect_uri, verifier는 challenge와 짝.
    let exchanged = endpoint.requests.lock().unwrap().clone();
    assert_eq!(exchanged.len(), 1);
    assert_eq!(exchanged[0].code, "c1");
    assert_eq!(
        exchanged[0].redirect_uri,
        "https://promptlab.sandcorp.co.kr/api/auth/callback"
    );
    assert_eq!(
        promptlab_server::auth::login::code_challenge(&exchanged[0].code_verifier),
        challenge
    );

    // 세션 쿠키로 API.
    let with_session = |method: &str, uri: &str| {
        Request::builder()
            .method(method)
            .uri(uri.to_owned())
            .header(header::COOKIE, format!("pl_session={session}"))
    };
    let (status, _, body) = send(&app, with_session("GET", "/api/me"), None).await;
    assert_eq!(status, StatusCode::OK);
    let me: MeView = serde_json::from_value(body).unwrap();
    assert_eq!(me.sub, ALICE);
    assert_eq!(me.name.as_deref(), Some("앨리스"));

    // 상태를 바꾸는 요청은 Origin이 이 서비스여야 한다(CSRF).
    let (status, _, _) = send(
        &app,
        with_session("POST", "/api/orgs"),
        Some(json!({"name": "에이"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "Origin 없음");
    let evil =
        with_session("POST", "/api/orgs").header(header::ORIGIN, "https://news.sandcorp.co.kr");
    let (status, _, _) = send(&app, evil, Some(json!({"name": "에이"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "다른 출처");
    let ours = with_session("POST", "/api/orgs").header(header::ORIGIN, PUBLIC_URL);
    let (status, _, _) = send(
        &app,
        ours,
        Some(json!({"name": "에이", "terms_version": TERMS_VERSION})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // 로그 아웃하면 세션이 끝난다.
    let logout = with_session("POST", "/api/auth/logout").header(header::ORIGIN, PUBLIC_URL);
    let (status, _, _) = send(&app, logout, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = send(&app, with_session("GET", "/api/me"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 로그인 상태는 한 번만 쓴다.
    let (status, headers) = callback(&app, &login_cookie, &format!("code=c1&state={state}")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(
        headers[header::LOCATION]
            .to_str()
            .unwrap()
            .contains("login_error")
    );
    assert!(
        set_cookies(&headers)
            .iter()
            .all(|c| !c.starts_with("pl_session="))
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 대시보드_로그인_실패_경우들은_세션을_주지_않는다(pool: PgPool) {
    let mut wrong_aud = member_claims(ALICE);
    wrong_aud["aud"] = json!("https://sandcorp.co.kr/mcp");
    for (what, response, tamper_state, auth_error) in [
        ("state 불일치", Ok(token_for(ALICE)), true, false),
        ("auth 오류", Ok(token_for(ALICE)), false, true),
        ("교환 실패", Err("invalid_grant".to_owned()), false, false),
        ("다른 aud 토큰", Ok(sign(&wrong_aud)), false, false),
        (
            "기계 토큰(#28)",
            Ok(machine_token(SMOKE_CLIENT)),
            false,
            false,
        ),
    ] {
        let endpoint = FakeTokenEndpoint::answering(response.map(|access_token| TokenResponse {
            access_token,
            id_token: None,
        }));
        let app = app_with(pool.clone(), endpoint);
        let (authorize, login_cookie) = start_login(&app).await;
        let state = if tamper_state {
            "other".to_owned()
        } else {
            query_param(&authorize, "state")
        };
        let query = if auth_error {
            format!("error=access_denied&state={state}")
        } else {
            format!("code=c1&state={state}")
        };
        let (status, headers) = callback(&app, &login_cookie, &query).await;
        assert_eq!(status, StatusCode::FOUND, "{what}");
        assert!(
            headers[header::LOCATION]
                .to_str()
                .unwrap()
                .contains("login_error"),
            "{what}"
        );
        assert!(
            set_cookies(&headers)
                .iter()
                .all(|c| !c.starts_with("pl_session=")),
            "{what}"
        );
    }

    // 로그인 쿠키 없이 콜백.
    let app = app(pool.clone());
    let (status, headers, _) = send(
        &app,
        Request::builder().uri("/auth/callback?code=c&state=s"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(
        set_cookies(&headers)
            .iter()
            .all(|c| !c.starts_with("pl_session="))
    );

    // 열린 리다이렉트 막기: next는 이 서비스 안의 경로만.
    for next in ["https://evil.example", "//evil.example", "/\\evil.example"] {
        let (_, headers, _) = send(
            &app,
            Request::builder().uri(format!("/auth/login?next={}", urlencode(next))),
            None,
        )
        .await;
        let _ = cookie_value(&headers, "pl_login");
    }
    assert_eq!(
        promptlab_server::auth::login::safe_next(Some("//evil.example")),
        "/"
    );
    assert_eq!(
        promptlab_server::auth::login::safe_next(Some("https://evil.example")),
        "/"
    );
    assert_eq!(
        promptlab_server::auth::login::safe_next(Some("/orgs/1")),
        "/orgs/1"
    );
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// 이용약관 동의(#86): 없음·옛 판은 422(초대는 쓰이지 않는다), 동의하면 기록이 응답과 `/me`에 보이고 다른 조직 사람은 못 본다.
#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 이용약관_동의가_없으면_422이고_동의하면_기록된다(pool: PgPool) {
    let app = app(pool.clone());
    for body in [
        json!({"name": "에이 회사"}),
        json!({"name": "에이 회사", "terms_version": "2020-01-01"}),
        json!({"name": "에이 회사", "terms_version": ""}),
    ] {
        let (status, _) = call(&app, "POST", "/orgs", Some(&token_for(ALICE)), Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let orgs: i64 = sqlx::query_scalar("SELECT count(*) FROM orgs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(orgs, 0, "동의 없이는 조직이 생기지 않는다");

    let org = create_org(&app, ALICE, "에이 회사").await;
    let terms = org.terms.clone().expect("만든 사람의 동의 기록");
    assert_eq!(terms.version, TERMS_VERSION);
    assert!(terms.accepted_at > 0);

    let (_, body) = call(
        &app,
        "POST",
        &format!("/orgs/{}/invites", org.id),
        Some(&token_for(ALICE)),
        Some(json!({"role": "viewer"})),
    )
    .await;
    let invite: InviteCreated = serde_json::from_value(body).unwrap();
    let accept = format!("/invites/{}/accept", invite.token);
    for body in [
        Some(json!({"terms_version": "2020-01-01"})),
        Some(json!({})),
        None,
    ] {
        let (status, _) = call(&app, "POST", &accept, Some(&token_for(BOB)), body).await;
        assert!(
            [
                StatusCode::UNPROCESSABLE_ENTITY,
                StatusCode::BAD_REQUEST,
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            ]
            .contains(&status),
            "{status}"
        );
    }
    let (status, _) = call(
        &app,
        "POST",
        &accept,
        Some(&token_for(BOB)),
        Some(json!({"terms_version": "2020-01-01"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM memberships WHERE sub = $1")
        .bind(BOB)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(members, 0, "옛 판이면 들어가지 않고 초대도 남는다");

    let (status, body) = call(
        &app,
        "POST",
        &accept,
        Some(&token_for(BOB)),
        Some(json!({"terms_version": TERMS_VERSION})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let joined: OrgView = serde_json::from_value(body).unwrap();
    assert_eq!(joined.terms.unwrap().version, TERMS_VERSION);

    // `/me`는 내 기록만 — 만든 사람과 들어온 사람 각자의 것.
    let (_, body) = call(&app, "GET", "/me", Some(&token_for(BOB)), None).await;
    let me: MeView = serde_json::from_value(body).unwrap();
    assert_eq!(me.orgs[0].terms.as_ref().unwrap().version, TERMS_VERSION);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM terms_acceptances WHERE org_id = $1::uuid")
            .bind(&org.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 2);

    // 다른 조직 사람(ERIN)은 이 조직도 기록도 못 본다.
    let (status, _) = call(
        &app,
        "GET",
        &format!("/orgs/{}", org.id),
        Some(&token_for(ERIN)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = call(&app, "GET", "/me", Some(&token_for(ERIN)), None).await;
    let me: MeView = serde_json::from_value(body).unwrap();
    assert!(me.orgs.is_empty());
}
