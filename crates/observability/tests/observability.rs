use promptlab_observability::*;
use sentry::protocol::{Event, Request};

const JWT: &str = "eyJhbGciOiJFZERTQSJ9.eyJzdWIiOiJtXzAxIn0.c2lnbmF0dXJl";

#[test]
fn dsn이_없거나_비면_켜지지_않는다() {
    assert!(init_with(None, None).is_none());
    assert!(init_with(Some("  "), Some("abc".into())).is_none());
    assert!(sentry::Hub::current().client().is_none());
}

#[test]
fn 잘못된_dsn은_켜지지_않는다() {
    assert!(init_with(Some("이건 dsn이 아니다"), None).is_none());
}

#[test]
fn 요청의_쿠키_인증_헤더_query를_지운다() {
    let events = sentry::test::with_captured_events_options(
        || {
            let mut req = Request {
                url: Some(
                    "https://promptlab.sandcorp.co.kr/api/x?token=secret&email=a@b.c"
                        .parse()
                        .unwrap(),
                ),
                query_string: Some("token=secret".into()),
                cookies: Some("pl_session=abc".into()),
                data: Some("본문".into()),
                ..Default::default()
            };
            for h in [
                "Cookie",
                "Authorization",
                "X-Api-Key",
                "X-Sandcorp-Signature",
            ] {
                req.headers.insert(h.into(), "비밀".into());
            }
            req.headers.insert("User-Agent".into(), "curl".into());
            sentry::capture_event(Event {
                request: Some(req),
                ..Default::default()
            });
        },
        client_options(Some("sha".into())),
    );
    let req = events[0].request.as_ref().unwrap();
    assert_eq!(req.headers.len(), 1, "{:?}", req.headers);
    assert!(req.headers.contains_key("User-Agent"));
    assert!(req.cookies.is_none() && req.query_string.is_none() && req.data.is_none());
    assert_eq!(req.url.as_ref().unwrap().query(), None);
    assert_eq!(events[0].release.as_deref(), Some("sha"));
    assert_eq!(events[0].environment.as_deref(), Some("production"));
}

#[test]
fn 메시지_예외_extra의_토큰_모양을_가린다() {
    let hex = "0123456789abcdef0123456789abcdef0123";
    let events = sentry::test::with_captured_events_options(
        || {
            let mut ev = Event {
                message: Some(format!("실패 Bearer {JWT} 끝")),
                ..Default::default()
            };
            ev.extra
                .insert("k".into(), format!("sk-ant-api03-AbC_d-1 {hex}").into());
            sentry::capture_event(ev);
            sentry::capture_error(&GlitchTipOnboardingTest(format!("토큰 {JWT}")));
        },
        client_options(None),
    );
    let m = events[0].message.as_deref().unwrap();
    assert!(
        !m.contains("eyJ") && m.contains(REDACTED) && m.starts_with("실패"),
        "{m}"
    );
    let extra = events[0].extra["k"].as_str().unwrap();
    assert!(!extra.contains("sk-ant") && !extra.contains(hex), "{extra}");
    let ex = events[1].exception.values[0].value.as_deref().unwrap();
    assert!(!ex.contains("eyJ"), "{ex}");
}

#[test]
fn 평범한_문장은_그대로() {
    assert_eq!(
        redact_str("DB 오류 — 잠시 뒤 다시"),
        "DB 오류 — 잠시 뒤 다시"
    );
}

#[test]
fn 시험_오류의_타입_이름이_보인다() {
    let events = sentry::test::with_captured_events_options(
        || {
            sentry::capture_error(&GlitchTipOnboardingTest(
                "연동 시험 promptlab 2026-11-07".into(),
            ));
        },
        client_options(None),
    );
    let ex = &events[0].exception.values[0];
    assert_eq!(ex.ty, "GlitchTipOnboardingTest");
    assert_eq!(ex.value.as_deref(), Some("연동 시험 promptlab 2026-11-07"));
}

#[test]
fn kst_날짜() {
    assert_eq!(kst_date(0), "1970-01-01");
    assert_eq!(kst_date(86_400 - 9 * 3600 - 1), "1970-01-01");
    assert_eq!(kst_date(86_400 - 9 * 3600), "1970-01-02");
    assert_eq!(kst_date(951_782_400), "2000-02-29"); // 2000-02-29 00:00 UTC
}

#[test]
fn 올바른_dsn이면_켜진다() {
    let g = init_with(
        Some("https://abc@glitchtip.example.invalid/1"),
        Some("sha".into()),
    );
    assert!(g.is_some());
}
