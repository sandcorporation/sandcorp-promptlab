//! 앱 서버 주소 기본값(#62): env 없는 release 앱은 운영 서버, 디버그는 개발 서버, 실행 env가 이긴다.

use promptlab_app::client::{Config, DEV_SERVER_URL, PRODUCTION_SERVER_URL, default_server_url};

#[test]
fn release_without_env_uses_production() {
    assert_eq!(
        default_server_url(true, None),
        "https://promptlab.sandcorp.co.kr/api"
    );
    assert_eq!(default_server_url(true, Some("  ")), PRODUCTION_SERVER_URL);
}

#[test]
fn release_baked_value_overrides_default() {
    assert_eq!(
        default_server_url(true, Some("https://staging.example/api")),
        "https://staging.example/api"
    );
}

#[test]
fn debug_uses_dev_server() {
    assert_eq!(default_server_url(false, None), DEV_SERVER_URL);
    assert_eq!(
        default_server_url(false, Some("https://x/api")),
        DEV_SERVER_URL
    );
}

#[test]
fn runtime_env_wins_and_default_applies_without_it() {
    let c = Config::from_lookup(|k| (k == "PROMPTLAB_SERVER_URL").then(|| "http://h:1/".into()))
        .unwrap();
    assert_eq!(c.server_url, "http://h:1");
    let c = Config::from_lookup(|_| None).unwrap();
    let expected = if cfg!(debug_assertions) {
        DEV_SERVER_URL
    } else {
        PRODUCTION_SERVER_URL
    };
    assert_eq!(c.server_url, expected);
}
