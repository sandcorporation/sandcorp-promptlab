//! 과정 로그(#9) HTTP API: 키 등록 → 서명한 사건 업로드(멱등·순서) → 변조·누락 거부와 신호 → 다시보기·격리.
//! 모든 응답에 숨은 입력 불변식을 건다(`call()`).

mod common;

use axum::http::StatusCode;
use common::*;
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamInvitesCreated, ExamView, InviteCreated, LogHead, OrgView, ProcessLogView,
    SubmissionState, SubmissionView,
};
use promptlab_shared::coding::Language;
use promptlab_shared::exam::Purpose;
use promptlab_shared::log::{
    Attempt, EditRange, GENESIS, LocalRunKind, LogEvent, LogSignal, LogSigner, SignedEvent,
    ToolCall, code_sha256, event_hash,
};
use promptlab_store::PgPool;
use serde_json::{Value, json};

async fn ok<T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
    expect: StatusCode,
) -> T {
    let (status, body) = call(app, method, uri, Some(&token_for(sub)), body).await;
    assert_eq!(status, expect, "{method} {uri}: {body}");
    serde_json::from_value(body).unwrap()
}

async fn status_of(
    app: &axum::Router,
    method: &str,
    uri: &str,
    sub: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call(app, method, uri, Some(&token_for(sub)), body).await
}

struct Exam {
    app: axum::Router,
    pool: PgPool,
    org: OrgView,
    exam: ExamView,
    assignment: AssignmentView,
}

impl Exam {
    fn attempt(&self) -> Attempt {
        Attempt {
            assignment_id: self.assignment.id.clone(),
            position: 0,
        }
    }

    fn item_uri(&self, tail: &str) -> String {
        format!("/assignments/{}/items/0/{tail}", self.assignment.id)
    }

    fn replay_uri(&self) -> String {
        format!(
            "/orgs/{}/exams/{}/invites/{}/items/0/log",
            self.org.id, self.exam.id, self.assignment.id
        )
    }

    async fn register(&self, signer: &LogSigner) -> LogHead {
        ok(
            &self.app,
            "POST",
            &self.item_uri("log/keys"),
            ERIN,
            Some(json!({"public_key": signer.public_key()})),
            StatusCode::OK,
        )
        .await
    }

    async fn upload(&self, events: &[SignedEvent]) -> (StatusCode, Value) {
        status_of(
            &self.app,
            "POST",
            &self.item_uri("log"),
            ERIN,
            Some(json!({ "events": events })),
        )
        .await
    }

    async fn replay(&self, sub: &str) -> ProcessLogView {
        ok(
            &self.app,
            "GET",
            &self.replay_uri(),
            sub,
            None,
            StatusCode::OK,
        )
        .await
    }
}

/// ALICE(관리자)의 조직에 BOB 출제자·CAROL 감독관·DAVE 열람자, ERIN이 배정받아 0번 문항을 시작했다.
async fn started(pool: PgPool) -> Exam {
    let app = app(pool.clone());
    let org: OrgView = ok(
        &app,
        "POST",
        "/orgs",
        ALICE,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    common::approve_org(&app, &org.id).await;
    for (sub, role) in [
        (BOB, Role::Author),
        (CAROL, Role::Proctor),
        (DAVE, Role::Viewer),
    ] {
        let invite: InviteCreated = ok(
            &app,
            "POST",
            &format!("/orgs/{}/invites", org.id),
            ALICE,
            Some(json!({ "role": role })),
            StatusCode::CREATED,
        )
        .await;
        let _: OrgView = ok(
            &app,
            "POST",
            &format!("/invites/{}/accept", invite.token),
            sub,
            Some(json!({"terms_version": promptlab_shared::terms::TERMS_VERSION})),
            StatusCode::OK,
        )
        .await;
    }
    let exam: ExamView = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(json!({
            "title": "프롬프트 역량 1차",
            "purpose": Purpose::Recruiting,
            "reveal_scores": false,
            "items": [{"problem_id": "count-vowels", "limits": {"time_limit_secs": 600}}],
        })),
        StatusCode::CREATED,
    )
    .await;
    let created: ExamInvitesCreated = ok(
        &app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        BOB,
        Some(json!({"emails": ["erin@example.com"]})),
        StatusCode::CREATED,
    )
    .await;
    let token = created.invites[0].token.clone().unwrap();
    let assignment: AssignmentView = ok(
        &app,
        "POST",
        &format!("/exam-invites/{token}/accept"),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    webcam_ready(&app, &token_for(ERIN), &assignment.id).await;
    let e = Exam {
        app,
        pool,
        org,
        exam,
        assignment,
    };
    let _: Value = ok(
        &e.app,
        "POST",
        &e.item_uri("start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    e
}

/// `head`부터 사건들을 이어 서명한다.
fn seal_from(
    signer: &LogSigner,
    attempt: &Attempt,
    head: &LogHead,
    events: Vec<LogEvent>,
) -> Vec<SignedEvent> {
    let mut prev = head.head.clone();
    events
        .into_iter()
        .enumerate()
        .map(|(i, event)| {
            let seq = head.next_seq + i as u64;
            let signed = signer.seal(attempt, seq, &prev, 1_767_225_600_000 + seq as i64, event);
            prev = event_hash(&signed.body);
            signed
        })
        .collect()
}

/// 제출 코드(가짜 판정기가 `correct`를 보고 모두 맞힌다).
const CODE: &str = "def max_subarray(nums):\n    return 0  # correct\n";

/// 코딩 한 문항: 시작 → 언어 고름 → 코드 스냅숏 → AI 도우미 질문·답(편집 제안) → 제안 수락 → 공개 예제 로컬 실행 →
/// 붙여넣기 막음 → 제출(언어·코드 sha256).
fn session_events() -> Vec<LogEvent> {
    vec![
        LogEvent::Started,
        LogEvent::LanguageChosen {
            language: Language::Python,
        },
        LogEvent::CodeSnapshot { code: CODE.into() },
        LogEvent::AiMessage {
            text: "경계 조건을 봐 줘".into(),
        },
        LogEvent::AiReply {
            text: "빈 배열을 확인하세요".into(),
            tool_calls: vec![ToolCall {
                name: "propose_edit".into(),
                arguments: json!({"proposal": 1}),
            }],
            stopped: false,
        },
        LogEvent::EditProposed {
            proposal: 1,
            range: EditRange {
                start_line: 2,
                start_column: 1,
                end_line: 2,
                end_column: 5,
            },
            text: "    if not nums: return 0\n".into(),
        },
        LogEvent::EditAccepted { proposal: 1 },
        LogEvent::LocalRun {
            kind: LocalRunKind::Examples,
            passed: Some(2),
            total: Some(3),
            summary: "2/3 통과".into(),
        },
        LogEvent::PasteBlocked { chars: 40 },
        LogEvent::Submitted {
            language: Language::Python,
            code_sha256: code_sha256(CODE),
        },
    ]
}

/// 사건 수.
fn n() -> usize {
    session_events().len()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 서명한_로그를_순서대로_받고_다시_보내도_멱등이며_감독관이_시간순으로_본다(
    pool: PgPool,
) {
    let e = started(pool).await;
    let signer = LogSigner::from_seed([1; 32]);
    let head = e.register(&signer).await;
    assert_eq!(head.next_seq, 0);
    assert_eq!(head.head, GENESIS);

    let events = seal_from(&signer, &e.attempt(), &head, session_events());
    // 오프라인으로 쌓였던 것을 두 번에 나눠 올린다.
    let (status, body) = e.upload(&events[..2]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = e.upload(&events).await; // 앞 둘은 다시 보낸 것(응답이 끊겼던 경우).
    assert_eq!(status, StatusCode::OK, "{body}");
    let head: LogHead = serde_json::from_value(body).unwrap();
    assert_eq!(head.next_seq, n() as u64);
    assert_eq!(head.head, event_hash(&events[n() - 1].body));
    // 같은 것을 또 보내도 그대로다.
    let (status, again) = e.upload(&events[3..]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again, serde_json::to_value(&head).unwrap());
    // 키를 다시 등록해도(앱을 다시 켬) 머리를 그대로 준다.
    assert_eq!(e.register(&signer).await, head);

    // 감독관·관리자·열람자가 본다. 출제자는 결과를 보지 않는다.
    for sub in [CAROL, ALICE, DAVE] {
        let view = e.replay(sub).await;
        assert_eq!(
            view.events
                .iter()
                .map(|r| r.event.clone())
                .collect::<Vec<_>>(),
            session_events(),
            "{sub}"
        );
        assert_eq!(
            view.events.iter().map(|r| r.seq).collect::<Vec<_>>(),
            (0..n() as u64).collect::<Vec<_>>()
        );
        assert!(
            view.events
                .iter()
                .all(|r| r.received_at == NOW && r.key_index == 0)
        );
        assert_eq!(view.started_at, Some(NOW));
        assert!(view.candidate.contains("erin@example.com"));
        assert_eq!(view.signals, vec![], "{sub}");
    }
    let (status, _) = status_of(&e.app, "GET", &e.replay_uri(), BOB, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 변조_누락_남의_키_업로드는_저장하지_않고_거부_신호로_남긴다(
    pool: PgPool,
) {
    let e = started(pool).await;
    let signer = LogSigner::from_seed([1; 32]);
    let head = e.register(&signer).await;
    let events = seal_from(&signer, &e.attempt(), &head, session_events());

    // 원문을 바꿨다 → 서명이 맞지 않는다.
    let mut forged = events[..3].to_vec();
    forged[2].body = forged[2].body.replace("return 0", "return 9");
    let (status, body) = e.upload(&forged).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("서명"));

    // 가운데를 뺐다 → 빈틈.
    let gapped = vec![events[0].clone(), events[2].clone()];
    let (status, body) = e.upload(&gapped).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // 등록하지 않은 키로 서명했다.
    let stranger = LogSigner::from_seed([2; 32]);
    let other = seal_from(&stranger, &e.attempt(), &head, vec![LogEvent::Started]);
    let (status, _) = e.upload(&other).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // 아무것도 저장되지 않았고(앞의 맞는 사건도) 거부가 신호로 남았다.
    let view = e.replay(CAROL).await;
    assert!(view.events.is_empty());
    match view.signals.as_slice() {
        [LogSignal::Rejected { count: 3, .. }] => {}
        other => panic!("거부 신호가 없다: {other:?}"),
    }

    // 맞는 것은 그다음에 받는다. 받은 순번을 다른 내용으로 다시 쓰려 하면 거부한다.
    let (status, _) = e.upload(&events[..2]).await;
    assert_eq!(status, StatusCode::OK);
    let rewritten = seal_from(
        &signer,
        &e.attempt(),
        &LogHead {
            next_seq: 1,
            head: event_hash(&events[0].body),
        },
        vec![LogEvent::CodeSnapshot {
            code: "바꿔 친 코드".into(),
        }],
    );
    let (status, body) = e.upload(&rewritten).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(e.replay(CAROL).await.events.len(), 2);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 다시_설치한_앱은_새_키로_서버_머리부터_잇고_키_교체가_보인다(
    pool: PgPool,
) {
    let e = started(pool).await;
    let old = LogSigner::from_seed([1; 32]);
    let head = e.register(&old).await;
    let first = seal_from(
        &old,
        &e.attempt(),
        &head,
        vec![
            LogEvent::Started,
            LogEvent::CodeSnapshot {
                code: "처음".into(),
            },
        ],
    );
    assert_eq!(e.upload(&first).await.0, StatusCode::OK);

    // 앱을 다시 설치했다: 새 키, 로컬 로그 없음. 등록하면 서버가 아는 머리를 받는다.
    let new = LogSigner::from_seed([3; 32]);
    let head = e.register(&new).await;
    assert_eq!(head.next_seq, 2);
    let rest = seal_from(&new, &e.attempt(), &head, vec![LogEvent::Started]);
    assert_eq!(e.upload(&rest).await.0, StatusCode::OK);

    let view = e.replay(CAROL).await;
    assert_eq!(
        view.events.iter().map(|r| r.key_index).collect::<Vec<_>>(),
        vec![0, 0, 1]
    );
    assert_eq!(view.signals, vec![LogSignal::KeyChanged { seq: 2 }]);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 제출은_로그의_마지막_제출_사건과_언어_코드가_같아야_받고_채점_뒤_통과_수만_보인다(
    pool: PgPool,
) {
    let e = started(pool.clone()).await;
    let signer = LogSigner::from_seed([1; 32]);
    let head = e.register(&signer).await;
    let events = seal_from(&signer, &e.attempt(), &head, session_events());
    assert_eq!(e.upload(&events[..2]).await.0, StatusCode::OK);
    let submit = |language: &str, code: &str, log_seq: usize| json!({"language": language, "code": code, "log_seq": log_seq});
    let uri = e.item_uri("submit");
    let token = token_for(ERIN);
    // 앱은 전부 썼다고 하는데 서버는 2개만 받았다 → 409(앱이 로그를 비우고 다시 낸다).
    let (status, body) = call(
        &e.app,
        "POST",
        &uri,
        Some(&token),
        Some(submit("python", CODE, n())),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    // 로그는 다 왔는데(앱이 2개라 한다) 그 안에 제출 사건이 없다 → 422.
    let (status, body) = call(
        &e.app,
        "POST",
        &uri,
        Some(&token),
        Some(submit("python", CODE, 2)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(e.upload(&events[2..]).await.0, StatusCode::OK);
    // 제출 사건과 코드·언어가 다르면 422. 빈 코드·너무 긴 코드도 422.
    let too_long = "#".repeat(promptlab_shared::coding::MAX_CODE_BYTES + 1);
    for body in [
        submit(
            "python",
            "def max_subarray(nums):\n    return 1  # correct\n",
            n(),
        ),
        submit("javascript", CODE, n()),
        submit("python", "  ", n()),
        submit("python", &too_long, n()),
    ] {
        let (status, got) = call(&e.app, "POST", &uri, Some(&token), Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{got}");
    }
    // 맞으면 받는다(응시자에게는 채용 시험이라 「응시 완료」만). 한 번만 낸다.
    let (status, body) = call(
        &e.app,
        "POST",
        &uri,
        Some(&token),
        Some(submit("python", CODE, n())),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let view: SubmissionView = serde_json::from_value(body).unwrap();
    let (status, _) = call(
        &e.app,
        "POST",
        &uri,
        Some(&token),
        Some(submit("python", CODE, n())),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(matches!(
        grade(&pool).await,
        promptlab_worker::Outcome::Graded(_)
    ));
    let mine: SubmissionView = ok(
        &e.app,
        "GET",
        &format!("/submissions/{}", view.id),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(mine.state, SubmissionState::Completed);
    // 결과 보기 역할은 통과 수만 본다(케이스별 결과·히든 테스트는 `call()` 불변식이 막는다).
    let theirs: SubmissionView = ok(
        &e.app,
        "GET",
        &format!("/submissions/{}", view.id),
        ALICE,
        None,
        StatusCode::OK,
    )
    .await;
    let SubmissionState::Done { report } = theirs.state else {
        panic!("채점 끝: {:?}", theirs.state);
    };
    assert_eq!(report.language, Language::Python);
    assert!(report.total > 0);
    assert_eq!(report.passed, report.total);
    // 로그가 다 왔고 제출 사건이 있으니 신호가 없다.
    assert_eq!(e.replay(CAROL).await.signals, vec![]);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 저장_뒤_바뀐_사건은_다시보기가_변조로_표시한다(pool: PgPool) {
    let e = started(pool).await;
    let signer = LogSigner::from_seed([1; 32]);
    let head = e.register(&signer).await;
    let events = seal_from(&signer, &e.attempt(), &head, session_events());
    assert_eq!(e.upload(&events).await.0, StatusCode::OK);

    sqlx::query(
        "UPDATE exam_log_events SET body = replace(body, 'return 0', 'return 9') WHERE seq = 2",
    )
    .execute(&e.pool)
    .await
    .unwrap();
    let view = e.replay(CAROL).await;
    assert_eq!(view.events.len(), 2, "어긋난 곳부터는 보이지 않는다");
    assert_eq!(view.signals[0], LogSignal::Tampered { seq: 2 });
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 남의_응시와_다른_조직은_보이지_않는다(pool: PgPool) {
    let e = started(pool).await;
    let signer = LogSigner::from_seed([1; 32]);

    // 배정받지 않은 사람은 그 배정에 키를 등록·업로드하지 못한다(404).
    let (status, _) = status_of(
        &e.app,
        "POST",
        &e.item_uri("log/keys"),
        CAROL,
        Some(json!({"public_key": signer.public_key()})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = status_of(
        &e.app,
        "POST",
        &e.item_uri("log"),
        CAROL,
        Some(json!({"events": []})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 응시자 본인도 조직의 다시보기는 못 본다(구성원이 아니다 → 404).
    let (status, _) = status_of(&e.app, "GET", &e.replay_uri(), ERIN, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 다른 조직의 관리자: 자기 조직 id로 이 응시를 찾아도 404.
    let other: OrgView = ok(
        &e.app,
        "POST",
        "/orgs",
        BOB,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    let uri = format!(
        "/orgs/{}/exams/{}/invites/{}/items/0/log",
        other.id, e.exam.id, e.assignment.id
    );
    let (status, _) = status_of(&e.app, "GET", &uri, BOB, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 없는 문항 번호도 404.
    let uri = e.replay_uri().replace("/items/0/", "/items/9/");
    let (status, _) = status_of(&e.app, "GET", &uri, CAROL, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 없는_문항과_이상한_키는_받지_않는다(pool: PgPool) {
    let e = started(pool).await;
    // 시험에는 문항이 하나뿐이다 — 없는 문항은 404.
    let (status, _) = status_of(
        &e.app,
        "POST",
        &e.item_uri("log/keys").replace("/items/0/", "/items/1/"),
        ERIN,
        Some(json!({"public_key": LogSigner::from_seed([1; 32]).public_key()})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = status_of(
        &e.app,
        "POST",
        &e.item_uri("log/keys"),
        ERIN,
        Some(json!({"public_key": "짧다"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
