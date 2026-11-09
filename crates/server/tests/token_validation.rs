//! sandcorp-auth `docs/resource-server-token-validation.md` 규칙과 1:1로 맞춘 시험.
//! 시험 이름 앞의 규칙 이름(B1·T5·A2·S1·J2 …)이 문서의 그 규칙이다. 고정 시계·가짜 JWKS를 쓴다.

mod common;

use common::*;
use promptlab_server::auth::token::{
    AccountKind, FetchError, Rejection, TokenValidator, bearer_token, has_scope, is_member_sub,
    parse_client_ids,
};
use serde_json::{Value, json};

const SCOPE: &str = "promptlab";

async fn check(validator: &TokenValidator, token: &str) -> Result<AccountKind, Rejection> {
    validator.validate(token, SCOPE).await.map(|v| v.kind)
}

fn with(mut claims: Value, key: &str, value: Value) -> Value {
    claims[key] = value;
    claims
}

fn without(mut claims: Value, key: &str) -> Value {
    claims.as_object_mut().unwrap().remove(key);
    claims
}

#[tokio::test]
async fn 통과_일반_사용자_토큰() {
    let (v, _, _) = validator();
    let verified = v.validate(&token_for(ALICE), SCOPE).await.unwrap();
    assert_eq!(verified.sub, ALICE);
    assert_eq!(verified.kind, AccountKind::Member);
    assert_eq!(verified.client_id, "app");
}

#[test]
fn b1_b2_bearer_헤더만_스킴_대소문자_무시() {
    assert_eq!(bearer_token(Some("Bearer abc")), Some("abc"));
    assert_eq!(bearer_token(Some("bearer abc")), Some("abc"));
    assert_eq!(bearer_token(Some("BEARER abc")), Some("abc"));
    assert_eq!(bearer_token(Some("Basic abc")), None);
    assert_eq!(bearer_token(Some("Bearer")), None);
    assert_eq!(bearer_token(Some("Bearer ")), None);
    assert_eq!(bearer_token(None), None);
}

#[tokio::test]
async fn t1_jws_세_부분이_아니거나_헤더가_깨지면_거부() {
    let (v, _, _) = validator();
    let good = token_for(ALICE);
    let parts: Vec<&str> = good.split('.').collect();
    for bad in [
        "abc".to_owned(),
        format!("{}.{}", parts[0], parts[1]),
        format!("{good}.x"),
        format!("!!!.{}.{}", parts[1], parts[2]),
        format!("bm90LWpzb24.{}.{}", parts[1], parts[2]), // "not-json"
    ] {
        assert_eq!(
            check(&v, &bad).await,
            Err(Rejection::InvalidToken("T1")),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn t2_alg가_eddsa가_아니면_거부() {
    let (v, _, _) = validator();
    let claims = member_claims(ALICE);
    for alg in [
        json!("none"),
        json!("HS256"),
        json!("RS256"),
        json!("ES256"),
        json!("eddsa"),
        Value::Null,
    ] {
        let token = sign_parts(&json!({"alg": alg, "kid": KID}), &claims, &key(1));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InvalidToken("T2")),
            "{alg}"
        );
    }
    let token = sign_parts(&json!({"kid": KID}), &claims, &key(1));
    assert_eq!(check(&v, &token).await, Err(Rejection::InvalidToken("T2")));
}

#[tokio::test]
async fn t3_kid가_문자열로_없으면_거부() {
    let (v, _, _) = validator();
    let claims = member_claims(ALICE);
    for header in [json!({"alg": "EdDSA"}), json!({"alg": "EdDSA", "kid": 1})] {
        let token = sign_parts(&header, &claims, &key(1));
        assert_eq!(check(&v, &token).await, Err(Rejection::InvalidToken("T3")));
    }
}

#[tokio::test]
async fn t4_모르는_kid는_거부() {
    let (v, _, _) = validator();
    let token = sign_parts(
        &json!({"alg": "EdDSA", "kid": "nope"}),
        &member_claims(ALICE),
        &key(1),
    );
    assert_eq!(check(&v, &token).await, Err(Rejection::InvalidToken("T4")));
}

#[tokio::test]
async fn t5_다른_키로_서명했거나_본문을_바꾸면_거부() {
    let (v, _, _) = validator();
    let other = sign_parts(
        &json!({"alg": "EdDSA", "kid": KID}),
        &member_claims(ALICE),
        &key(2),
    );
    assert_eq!(check(&v, &other).await, Err(Rejection::InvalidToken("T5")));

    // 서명은 그대로 두고 본문(sub)을 바꿔치기.
    let good = token_for(ALICE);
    let forged = token_for(BOB);
    let g: Vec<&str> = good.split('.').collect();
    let f: Vec<&str> = forged.split('.').collect();
    let swapped = format!("{}.{}.{}", g[0], f[1], g[2]);
    assert_eq!(
        check(&v, &swapped).await,
        Err(Rejection::InvalidToken("T5"))
    );
}

#[tokio::test]
async fn t6_필수_클레임이_하나라도_없으면_거부() {
    let (v, _, _) = validator();
    for claim in [
        "iss",
        "sub",
        "aud",
        "client_id",
        "scope",
        "iat",
        "exp",
        "jti",
    ] {
        let token = sign(&without(member_claims(ALICE), claim));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InvalidToken("T6")),
            "{claim}"
        );
    }
}

#[tokio::test]
async fn t7_타입이_다르면_거부() {
    let (v, _, _) = validator();
    let cases = [
        ("aud", json!([AUDIENCE])),
        ("iat", json!("1767225600")),
        ("exp", json!(1767226200.5)),
        ("sub", json!(1)),
        ("scope", json!(["promptlab"])),
        ("client_id", Value::Null),
        ("jti", json!(7)),
        ("iss", json!({"x": 1})),
    ];
    for (claim, value) in cases {
        let token = sign(&with(member_claims(ALICE), claim, value.clone()));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InvalidToken("T7")),
            "{claim}={value}"
        );
    }
}

#[tokio::test]
async fn t8_iss가_정확히_같지_않으면_거부() {
    let (v, _, _) = validator();
    for iss in [
        "https://auth.sandcorp.co.kr/",
        "https://AUTH.sandcorp.co.kr",
        "https://evil.example",
    ] {
        let token = sign(&with(member_claims(ALICE), "iss", json!(iss)));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InvalidToken("T8")),
            "{iss}"
        );
    }
}

#[tokio::test]
async fn t9_aud가_자기_리소스와_정확히_같지_않으면_거부() {
    let (v, _, _) = validator();
    for aud in [
        "https://promptlab.sandcorp.co.kr/api/",
        "https://PROMPTLAB.sandcorp.co.kr/api",
        "https://promptlab.sandcorp.co.kr",
        "https://sandcorp.co.kr/mcp",
        "https://auth.sandcorp.co.kr/userinfo",
    ] {
        let token = sign(&with(member_claims(ALICE), "aud", json!(aud)));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InvalidToken("T9")),
            "{aud}"
        );
    }
}

#[tokio::test]
async fn t10_exp_더하기_30초가_지나면_거부_iat_nbf는_보지_않는다() {
    let (v, _, clock) = validator();
    let token = token_for(ALICE);
    clock.advance(600 + 30);
    assert!(check(&v, &token).await.is_ok(), "exp+30 까지는 통과");
    clock.advance(1);
    assert_eq!(check(&v, &token).await, Err(Rejection::InvalidToken("T10")));

    // 미래의 iat·nbf는 시각 비교를 하지 않는다.
    let (v, _, _) = validator();
    let future = with(member_claims(ALICE), "iat", json!(NOW + 3600));
    let future = with(future, "nbf", json!(NOW + 3600));
    assert!(check(&v, &sign(&future)).await.is_ok());
}

#[tokio::test]
async fn a1_acct가_없으면_대표_토큰이고_sub가_대표여야_한다() {
    let (v, _, _) = validator();
    let owner = without(with(member_claims(ALICE), "sub", json!(OWNER_SUB)), "acct");
    assert_eq!(check(&v, &sign(&owner)).await, Ok(AccountKind::Owner));
    for sub in [ALICE, "Farrar142", " farrar142", "someone"] {
        let token = sign(&without(
            with(member_claims(ALICE), "sub", json!(sub)),
            "acct",
        ));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InvalidToken("A1")),
            "{sub}"
        );
    }
}

#[tokio::test]
async fn a2_acct가_있는데_member가_아니면_거부() {
    let (v, _, _) = validator();
    for acct in [
        json!("owner"),
        json!("Member"),
        json!(""),
        json!(["member"]),
        Value::Null,
    ] {
        let token = sign(&with(member_claims(ALICE), "acct", acct.clone()));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InvalidToken("A2")),
            "{acct}"
        );
    }
}

#[tokio::test]
async fn a3_member인데_sub가_ulid_모양이_아니면_거부() {
    let (v, _, _) = validator();
    for sub in [
        OWNER_SUB,
        "m_01jb2z3y4x5w6v7t8s9r0q1p2n",
        "m_81JB2Z3Y4X5W6V7T8S9R0Q1P2N",
        "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2",
    ] {
        let token = sign(&with(member_claims(ALICE), "sub", json!(sub)));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InvalidToken("A3")),
            "{sub}"
        );
    }
}

#[test]
fn a3_정규식() {
    assert!(is_member_sub("m_01JB2Z3Y4X5W6V7T8S9R0Q1P2N"));
    assert!(is_member_sub("m_7ZZZZZZZZZZZZZZZZZZZZZZZZZ"));
    for bad in [
        "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2I",
        "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2L",
        "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2O",
        "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2U",
        "M_01JB2Z3Y4X5W6V7T8S9R0Q1P2N",
        "m_01JB2Z3Y4X5W6V7T8S9R0Q1P2NN",
        "",
    ] {
        assert!(!is_member_sub(bad), "{bad}");
    }
}

#[tokio::test]
async fn s1_요구_스코프가_원소로_없으면_403() {
    let (v, _, _) = validator();
    for scope in ["login", "promptlab.use", "promptlabx", "admin editor", ""] {
        let token = sign(&with(member_claims(ALICE), "scope", json!(scope)));
        assert_eq!(
            check(&v, &token).await,
            Err(Rejection::InsufficientScope),
            "{scope}"
        );
    }
    let token = sign(&with(
        member_claims(ALICE),
        "scope",
        json!("  login  promptlab "),
    ));
    assert!(check(&v, &token).await.is_ok());
    assert!(!has_scope("administrator", "admin"));
}

#[tokio::test]
async fn j1_캐시는_600초_뒤에_다시_받는다() {
    let (v, jwks, clock) = validator();
    let token = token_for(ALICE);
    check(&v, &token).await.unwrap();
    check(&v, &token).await.unwrap();
    assert_eq!(jwks.calls(), 1);
    clock.advance(599);
    check(
        &v,
        &sign(&with(member_claims(ALICE), "exp", json!(NOW + 2000))),
    )
    .await
    .unwrap();
    assert_eq!(jwks.calls(), 1);
    clock.advance(1);
    check(
        &v,
        &sign(&with(member_claims(ALICE), "exp", json!(NOW + 2000))),
    )
    .await
    .unwrap();
    assert_eq!(jwks.calls(), 2);
}

#[tokio::test]
async fn j2_모르는_kid는_한_번_다시_받되_60초에_한_번만() {
    let (v, jwks, clock) = validator();
    check(&v, &token_for(ALICE)).await.unwrap();
    assert_eq!(jwks.calls(), 1);

    // 키 회전(J5): 새 kid가 JWKS에 나타난다 → 그 자리에서 다시 받아 통과.
    jwks.set(Ok(jwks_body(&[("key-1", 1), ("key-2", 2)])));
    let rotated = sign_parts(
        &json!({"alg": "EdDSA", "kid": "key-2"}),
        &member_claims(ALICE),
        &key(2),
    );
    assert!(check(&v, &rotated).await.is_ok());
    assert_eq!(jwks.calls(), 2);

    // 60초 안의 또 다른 모르는 kid는 다시 받지 않고 거부.
    let unknown = sign_parts(
        &json!({"alg": "EdDSA", "kid": "key-3"}),
        &member_claims(ALICE),
        &key(3),
    );
    assert_eq!(
        check(&v, &unknown).await,
        Err(Rejection::InvalidToken("T4"))
    );
    assert_eq!(jwks.calls(), 2);
    clock.advance(60);
    assert_eq!(
        check(&v, &unknown).await,
        Err(Rejection::InvalidToken("T4"))
    );
    assert_eq!(jwks.calls(), 3);
}

fn jwks_body(keys: &[(&str, u8)]) -> String {
    jwks(
        &keys
            .iter()
            .map(|(kid, seed)| jwk(kid, &key(*seed)))
            .collect::<Vec<_>>(),
    )
}

#[tokio::test]
async fn j3_ed25519_서명용_키만_쓰고_조회는_캐시를_통째로_바꾼다() {
    let (v, fake, clock) = validator();
    let mut wrong_use = jwk("use-enc", &key(4));
    wrong_use["use"] = json!("enc");
    let mut wrong_alg = jwk("alg-rs", &key(5));
    wrong_alg["alg"] = json!("RS256");
    let mut wrong_crv = jwk("crv-x", &key(6));
    wrong_crv["crv"] = json!("X25519");
    let mut no_optional = jwk("bare", &key(7));
    no_optional.as_object_mut().unwrap().remove("alg");
    no_optional.as_object_mut().unwrap().remove("use");
    fake.set(Ok(jwks(&[wrong_use, wrong_alg, wrong_crv, no_optional])));

    let token = |kid: &str, seed: u8| {
        sign_parts(
            &json!({"alg": "EdDSA", "kid": kid}),
            &member_claims(ALICE),
            &key(seed),
        )
    };
    assert!(
        check(&v, &token("bare", 7)).await.is_ok(),
        "alg·use가 없어도 쓴다"
    );
    for (kid, seed) in [("use-enc", 4), ("alg-rs", 5), ("crv-x", 6)] {
        clock.advance(60);
        assert_eq!(
            check(&v, &token(kid, seed)).await,
            Err(Rejection::InvalidToken("T4")),
            "{kid}"
        );
    }
    // JWKS에서 빠진 키(key-1)는 캐시에서도 빠졌다.
    clock.advance(60);
    assert_eq!(
        check(&v, &token_for(ALICE)).await,
        Err(Rejection::InvalidToken("T4"))
    );
}

#[tokio::test]
async fn j4_조회가_실패하면_기존_캐시를_쓰고_60초_동안_다시_시도하지_않는다() {
    let (v, jwks, clock) = validator();
    check(&v, &token_for(ALICE)).await.unwrap();
    for failure in [
        Err(FetchError("타임아웃".into())),
        Ok("{not json".to_owned()),
    ] {
        jwks.set(failure);
        clock.advance(600);
        let before = jwks.calls();
        let token = sign(&with(member_claims(ALICE), "exp", json!(NOW + 100_000)));
        assert!(check(&v, &token).await.is_ok(), "실패해도 기존 키로 통과");
        assert_eq!(jwks.calls(), before + 1);
        clock.advance(59);
        assert!(check(&v, &token).await.is_ok());
        assert_eq!(jwks.calls(), before + 1, "60초 안에는 다시 시도하지 않는다");
        clock.advance(1);
    }

    // 처음부터 조회가 안 되면 키가 없으니 T4.
    let (v, jwks, _) = validator();
    jwks.set(Err(FetchError("연결 실패".into())));
    assert_eq!(
        check(&v, &token_for(ALICE)).await,
        Err(Rejection::InvalidToken("T4"))
    );
}

// ── 12절 기계 클라이언트 토큰(T1~T10 그대로, T11 대신 M1~M3, 그다음 S1) ──

#[tokio::test]
async fn 통과_허용된_기계_클라이언트_토큰() {
    let (v, _, _) = validator();
    let verified = v
        .validate(&machine_token(SMOKE_CLIENT), SCOPE)
        .await
        .unwrap();
    assert_eq!(verified.sub, format!("client:{SMOKE_CLIENT}"));
    assert_eq!(verified.kind, AccountKind::Machine);
    assert_eq!(verified.client_id, SMOKE_CLIENT);
}

#[tokio::test]
async fn m1_acct가_정확히_machine이_아니면_거부() {
    let (v, _, _) = validator();
    let claims = machine_claims(SMOKE_CLIENT);
    for (acct, rule) in [
        (json!("Machine"), "A2"),
        (json!("MACHINE"), "A2"),
        (json!("machine "), "A2"),
        (json!(true), "A2"),
        (json!(["machine"]), "A2"),
        (json!("member"), "A3"), // member 규칙으로 가 sub 꼴에서 걸린다
    ] {
        assert_eq!(
            check(&v, &sign(&with(claims.clone(), "acct", acct.clone()))).await,
            Err(Rejection::InvalidToken(rule)),
            "acct {acct}"
        );
    }
    // acct 없음 = 대표 규칙(A1) — client: sub는 대표가 아니다.
    assert_eq!(
        check(&v, &sign(&without(claims, "acct"))).await,
        Err(Rejection::InvalidToken("A1"))
    );
}

#[tokio::test]
async fn m2_sub가_client_머리_더하기_client_id가_아니면_거부() {
    let (v, _, _) = validator();
    let claims = machine_claims(SMOKE_CLIENT);
    for sub in [
        format!("client:{OTHER_MACHINE}"),
        SMOKE_CLIENT.to_owned(),
        format!("Client:{SMOKE_CLIENT}"),
        format!("client:{SMOKE_CLIENT} "),
        format!("client: {SMOKE_CLIENT}"),
        ALICE.to_owned(),
        OWNER_SUB.to_owned(),
    ] {
        assert_eq!(
            check(&v, &sign(&with(claims.clone(), "sub", json!(sub)))).await,
            Err(Rejection::InvalidToken("M2")),
            "sub {sub}"
        );
    }
}

#[tokio::test]
async fn m3_허용_목록에_없는_기계_클라이언트는_거부() {
    let (v, _, _) = validator();
    assert_eq!(
        check(&v, &machine_token(OTHER_MACHINE)).await,
        Err(Rejection::InvalidToken("M3"))
    );
    // 대소문자도 정확히.
    assert_eq!(
        check(&v, &machine_token(&SMOKE_CLIENT.to_uppercase())).await,
        Err(Rejection::InvalidToken("M3"))
    );
}

#[tokio::test]
async fn m3_허용_목록이_비면_기계_토큰은_전부_거부() {
    let mut rules = rules();
    rules.machine_client_ids.clear();
    let fetcher = FakeJwks::serving(jwks(&[jwk(KID, &key(1))]));
    let v = TokenValidator::new(rules, fetcher, FixedClock::at(NOW));
    assert_eq!(
        check(&v, &machine_token(SMOKE_CLIENT)).await,
        Err(Rejection::InvalidToken("M3"))
    );
    // 사람 토큰은 그대로.
    assert_eq!(check(&v, &token_for(ALICE)).await, Ok(AccountKind::Member));
}

#[tokio::test]
async fn m_다음_s1_기계_토큰도_스코프가_없으면_403() {
    let (v, _, _) = validator();
    let claims = with(machine_claims(SMOKE_CLIENT), "scope", json!("other"));
    assert_eq!(
        check(&v, &sign(&claims)).await,
        Err(Rejection::InsufficientScope)
    );
}

#[tokio::test]
async fn m_앞_t1_t10은_그대로_기계_토큰도_aud_exp를_본다() {
    let (v, _, _) = validator();
    let claims = machine_claims(SMOKE_CLIENT);
    assert_eq!(
        check(
            &v,
            &sign(&with(
                claims.clone(),
                "aud",
                json!("https://other.example/api")
            ))
        )
        .await,
        Err(Rejection::InvalidToken("T9"))
    );
    assert_eq!(
        check(&v, &sign(&with(claims, "exp", json!(NOW - 31)))).await,
        Err(Rejection::InvalidToken("T10"))
    );
}

#[test]
fn m3_허용_목록_env_꼴은_쉼표_구분_공백_빈_조각_무시() {
    assert!(parse_client_ids("").is_empty());
    assert!(parse_client_ids(" , ,").is_empty());
    assert_eq!(
        parse_client_ids(" a, b ,,a"),
        ["a".to_owned(), "b".to_owned()].into()
    );
}
