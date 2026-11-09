//! 잠금·무결성(#12)의 순수 규칙: 허용 목록 대조, 의심 신호 설명.

use promptlab_shared::inference::{LARGEST_MODEL as MODEL, MODELS};
use promptlab_shared::integrity::{
    ALLOWED_APP_VERSIONS, ALLOWED_MODEL_SHA256, AllowAction, AllowKind, Allowlist, ClientIntegrity,
    SuspicionKind, check, validate_change,
};

fn report(app: &str, model: &str) -> ClientIntegrity {
    ClientIntegrity {
        app_version: app.into(),
        model_sha256: model.into(),
    }
}

#[test]
fn 지금_판의_앱과_고정_모델은_허용_목록에_있다() {
    // 워크스페이스 판을 올리고 목록을 안 고치면 새 앱이 모두 막힌다 — 여기서 잡는다.
    assert!(ALLOWED_APP_VERSIONS.contains(&env!("CARGO_PKG_VERSION")));
    // 후보 모델(#131) 전부가 허용 목록이다.
    let all: Vec<_> = MODELS.iter().map(|m| m.sha256).collect();
    assert_eq!(ALLOWED_MODEL_SHA256, all.as_slice());
    for m in &MODELS {
        assert_eq!(
            check(Some(&report(env!("CARGO_PKG_VERSION"), m.sha256))),
            Ok(())
        );
    }
    assert_eq!(
        check(Some(&report(env!("CARGO_PKG_VERSION"), MODEL.sha256))),
        Ok(())
    );
    // 대문자 16진도 같은 해시다.
    assert_eq!(
        check(Some(&report(
            env!("CARGO_PKG_VERSION"),
            &MODEL.sha256.to_ascii_uppercase()
        ))),
        Ok(())
    );
}

#[test]
fn 목록에_없는_판_모델_신고_없음은_거부하고_무엇이_틀렸는지_가린다() {
    let other = "f".repeat(64);
    let e = check(Some(&report("9.9.9", MODEL.sha256))).unwrap_err();
    assert!((!e.app_ok && e.model_ok));
    assert!(e.message().contains("앱 판"));
    let e = check(Some(&report(ALLOWED_APP_VERSIONS[0], &other))).unwrap_err();
    assert!((e.app_ok && !e.model_ok));
    assert!(e.message().contains("모델"));
    let e = check(None).unwrap_err();
    assert!(!e.app_ok && !e.model_ok);
}

#[test]
fn 붙여넣기_막음_신호는_글자_수를_말한다() {
    let kind = SuspicionKind::PasteBlocked { seq: 3, chars: 40 };
    assert_eq!(kind.label(), "붙여넣기 막음");
    assert!(kind.describe().contains("40자"));
    assert_eq!(
        serde_json::to_value(&kind).unwrap(),
        serde_json::json!({"type": "paste_blocked", "seq": 3, "chars": 40})
    );
}

#[test]
fn 운영자_변경은_상수에_더하고_빼며_되돌리면_상수_그대로다() {
    let current = ALLOWED_APP_VERSIONS[0].to_owned();
    let list = Allowlist::with_overrides(&[
        (AllowKind::AppVersion, "9.9.9".into(), AllowAction::Allow),
        (AllowKind::AppVersion, current.clone(), AllowAction::Block),
    ]);
    assert_eq!(list.app_versions, vec!["9.9.9".to_owned()]);
    let all: Vec<_> = MODELS.iter().map(|m| m.sha256.to_owned()).collect();
    assert_eq!(list.model_sha256, all);
    assert!(list.check(Some(&report("9.9.9", MODEL.sha256))).is_ok());
    let e = list
        .check(Some(&report(&current, MODEL.sha256)))
        .unwrap_err();
    assert!(!e.app_ok && e.model_ok);
    // 되돌리기는 상수 기본값.
    let reset = Allowlist::with_overrides(&[(AllowKind::AppVersion, current, AllowAction::Reset)]);
    assert_eq!(reset, Allowlist::constants());
    // 한 모델을 막으면 그 모델의 신고만 모델 실패 — 다른 후보는 그대로 받는다.
    let blocked = Allowlist::with_overrides(&[(
        AllowKind::ModelSha256,
        MODEL.sha256.into(),
        AllowAction::Block,
    )]);
    let e = blocked
        .check(Some(&report(ALLOWED_APP_VERSIONS[0], MODEL.sha256)))
        .unwrap_err();
    assert!(e.app_ok && !e.model_ok);
    assert!(
        blocked
            .check(Some(&report(ALLOWED_APP_VERSIONS[0], MODELS[1].sha256)))
            .is_ok()
    );
}

#[test]
fn 변경_검사는_모델_더하기와_틀린_형식을_거부하고_해시는_소문자로_둔다() {
    assert!(validate_change(AllowKind::ModelSha256, MODEL.sha256, AllowAction::Allow).is_err());
    assert_eq!(
        validate_change(
            AllowKind::ModelSha256,
            &MODEL.sha256.to_ascii_uppercase(),
            AllowAction::Block
        ),
        Ok(MODEL.sha256.to_owned())
    );
    assert!(validate_change(AllowKind::ModelSha256, "abc", AllowAction::Block).is_err());
    assert_eq!(
        validate_change(AllowKind::AppVersion, " 0.2.0-rc.1 ", AllowAction::Allow),
        Ok("0.2.0-rc.1".to_owned())
    );
    for bad in ["", "0.1 0", "0.1.0;drop", &"1".repeat(65)] {
        assert!(
            validate_change(AllowKind::AppVersion, bad, AllowAction::Allow).is_err(),
            "{bad}"
        );
    }
}

/// 「쓴 모델」(#131): 문항 시작 신고 sha256 → 후보 이름(대소문자 무시), 후보 밖이면 앞 12자를 붙인 「알 수 없는 모델」.
#[test]
fn 신고한_sha256을_모델_이름으로_푼다() {
    use promptlab_shared::inference::{MODELS, model_name};
    for m in MODELS {
        assert_eq!(model_name(m.sha256), m.label);
        assert_eq!(model_name(&m.sha256.to_uppercase()), m.label);
    }
    assert_eq!(
        model_name(&"ab".repeat(32)),
        "알 수 없는 모델 (abababababab…)"
    );
}
