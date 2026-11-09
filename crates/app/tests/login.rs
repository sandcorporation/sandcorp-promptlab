//! 응시 앱 로그인 흐름(공개 클라이언트 + PKCE + 루프백). 가짜 auth·서버를 띄우고, 브라우저 대신
//! 인가 URL을 읽어 루프백 리다이렉트를 직접 부른다.

use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use promptlab_app::auth::code_challenge;
use promptlab_app::client::{ApiClient, ClientError, Config};
use promptlab_shared::api::{AppAuthConfig, SubmissionState, SubmissionView};
use reqwest::Url;
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    base: String,
    /// 브라우저가 연 인가 URL의 code_challenge.
    challenge: Mutex<Option<String>>,
    token_forms: Mutex<Vec<Vec<(String, String)>>>,
    bearers: Mutex<Vec<String>>,
    /// 첫 토큰의 수명(짧으면 다음 호출에서 리프레시).
    first_expires_in: u64,
}

async fn app_config(State(fake): State<Arc<Fake>>) -> Json<AppAuthConfig> {
    Json(AppAuthConfig {
        authorization_endpoint: format!("{}/authorize", fake.base),
        token_endpoint: format!("{}/token", fake.base),
        client_id: "https://promptlab.sandcorp.co.kr/api/oauth/app-client.json".into(),
        resource: "https://promptlab.sandcorp.co.kr/api".into(),
        scope: "promptlab".into(),
    })
}

async fn token(
    State(fake): State<Arc<Fake>>,
    Form(form): Form<Vec<(String, String)>>,
) -> (StatusCode, Json<Value>) {
    let get = |k: &str| {
        form.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
    };
    fake.token_forms.lock().unwrap().push(form.clone());
    match get("grant_type").as_deref() {
        Some("authorization_code") => {
            let verifier = get("code_verifier").unwrap_or_default();
            let expected = fake.challenge.lock().unwrap().clone();
            if get("code").as_deref() != Some("the-code")
                || Some(code_challenge(&verifier)) != expected
            {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "invalid_grant"})),
                );
            }
            (
                StatusCode::OK,
                Json(
                    json!({"access_token": "at-1", "token_type": "Bearer", "expires_in": fake.first_expires_in, "refresh_token": "rt-1"}),
                ),
            )
        }
        Some("refresh_token") if get("refresh_token").as_deref() == Some("rt-1") => (
            StatusCode::OK,
            Json(
                json!({"access_token": "at-2", "token_type": "Bearer", "expires_in": 600, "refresh_token": "rt-2"}),
            ),
        ),
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        ),
    }
}

async fn submission(State(fake): State<Arc<Fake>>, headers: HeaderMap) -> Json<SubmissionView> {
    let bearer = headers["authorization"].to_str().unwrap().to_owned();
    fake.bearers.lock().unwrap().push(bearer);
    Json(SubmissionView {
        id: "s1".into(),
        problem_id: "p".into(),
        org_id: None,
        state: SubmissionState::Queued,
        score: None,
    })
}

async fn serve(first_expires_in: u64) -> Arc<Fake> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake = Arc::new(Fake {
        base: format!("http://{}", listener.local_addr().unwrap()),
        first_expires_in,
        ..Fake::default()
    });
    let router = Router::new()
        .route("/auth/app-config", get(app_config))
        .route("/token", post(token))
        .route("/submissions/{id}", get(submission))
        .with_state(fake.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    fake
}

fn config(fake: &Fake, access_token: Option<&str>) -> Config {
    let base = fake.base.clone();
    let access_token = access_token.map(str::to_owned);
    Config::from_lookup(move |k| match k {
        "PROMPTLAB_SERVER_URL" => Some(base.clone()),
        "PROMPTLAB_ACCESS_TOKEN" => access_token.clone(),
        _ => None,
    })
    .unwrap()
}

/// 브라우저 흉내: 인가 URL을 확인하고 루프백으로 `code`(와 `state`)를 돌려보낸다.
fn browser(
    fake: Arc<Fake>,
    opened: Arc<Mutex<Vec<String>>>,
    tamper_state: bool,
) -> promptlab_app::client::BrowserOpener {
    Arc::new(move |url: &str| {
        opened.lock().unwrap().push(url.to_owned());
        let url = Url::parse(url).unwrap();
        let q = |k: &str| {
            url.query_pairs()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
                .unwrap()
        };
        assert_eq!(q("response_type"), "code");
        assert_eq!(q("code_challenge_method"), "S256");
        assert_eq!(q("resource"), "https://promptlab.sandcorp.co.kr/api");
        assert_eq!(q("scope"), "promptlab");
        assert_eq!(
            q("client_id"),
            "https://promptlab.sandcorp.co.kr/api/oauth/app-client.json"
        );
        let redirect = q("redirect_uri");
        assert!(
            redirect.starts_with("http://127.0.0.1:") && redirect.ends_with("/callback"),
            "{redirect}"
        );
        *fake.challenge.lock().unwrap() = Some(q("code_challenge"));
        let state = if tamper_state {
            "다른-state".to_owned()
        } else {
            q("state")
        };
        tokio::spawn(async move {
            let back = format!("{redirect}?code=the-code&state={state}");
            let page = reqwest::get(back).await.unwrap().text().await.unwrap();
            assert!(page.contains("로그인"));
        });
    })
}

#[tokio::test]
async fn 처음_부르면_브라우저로_로그인하고_만료_전에_리프레시한다() {
    let fake = serve(30).await; // 30초 — 1분 여유보다 짧아 다음 호출이 리프레시한다.
    let opened = Arc::new(Mutex::new(Vec::new()));
    let api = ApiClient::with_browser(
        config(&fake, None),
        browser(fake.clone(), opened.clone(), false),
    )
    .unwrap();

    api.submission("s1").await.unwrap();
    api.submission("s1").await.unwrap();
    api.submission("s1").await.unwrap();

    assert_eq!(opened.lock().unwrap().len(), 1, "브라우저는 한 번만");
    assert_eq!(
        *fake.bearers.lock().unwrap(),
        vec!["Bearer at-1", "Bearer at-2", "Bearer at-2"]
    );
    let forms = fake.token_forms.lock().unwrap();
    assert_eq!(forms.len(), 2);
    let get = |form: &Vec<(String, String)>, k: &str| {
        form.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
    };
    // 코드 교환: 시크릿 없이 client_id·code_verifier, redirect_uri는 인가 요청과 같은 값(포트 포함).
    assert_eq!(
        get(&forms[0], "grant_type").as_deref(),
        Some("authorization_code")
    );
    assert!(get(&forms[0], "client_secret").is_none());
    assert!(
        get(&forms[0], "redirect_uri")
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    assert_eq!(
        get(&forms[1], "grant_type").as_deref(),
        Some("refresh_token")
    );
}

#[tokio::test]
async fn state가_다르면_로그인하지_않는다() {
    let fake = serve(600).await;
    let opened = Arc::new(Mutex::new(Vec::new()));
    let api =
        ApiClient::with_browser(config(&fake, None), browser(fake.clone(), opened, true)).unwrap();
    let err = api.submission("s1").await.unwrap_err();
    assert!(matches!(err, ClientError::Login(_)), "{err}");
    assert!(
        fake.token_forms.lock().unwrap().is_empty(),
        "코드를 교환하지 않는다"
    );
    assert!(fake.bearers.lock().unwrap().is_empty());
}

#[tokio::test]
async fn 액세스_토큰을_주면_로그인하지_않고_그대로_쓴다() {
    let fake = serve(600).await;
    let opened = Arc::new(Mutex::new(Vec::new()));
    let api = ApiClient::with_browser(
        config(&fake, Some("given")),
        browser(fake.clone(), opened.clone(), false),
    )
    .unwrap();
    api.submission("s1").await.unwrap();
    assert!(opened.lock().unwrap().is_empty());
    assert_eq!(*fake.bearers.lock().unwrap(), vec!["Bearer given"]);
}
