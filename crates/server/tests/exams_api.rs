//! 시험 이음매 ②: 시험·응시 배정 초대·응시(#8). 라우터에 프로세스 안에서 요청을 보낸다.
//! 서버 시계는 고정 시계(`FixedClock`)로 움직여 제한 시간을 시험하고, 메일은 가짜 발송기로 받는다.
//! 모든 응답에 숨은 입력 불변식이 걸린다(`common::send`).

mod common;

use std::sync::{Arc, Mutex};

use axum::http::{Request, StatusCode, header};
use common::*;
use promptlab_server::auth::token::BoxFuture;
use promptlab_server::mail::{Mail, MailError, Mailer};
use promptlab_shared::account::Role;
use promptlab_shared::api::{
    AssignmentView, ExamInviteView, ExamInvitesCreated, ExamView, InviteCreated, ItemStarted,
    ItemState, OrgView, SubmissionState, SubmissionView,
};
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_shared::exam::{Delivery, InviteStatus, Purpose, SUBMIT_GRACE_SECS};
use promptlab_store::PgPool;
/// 공개 픽스처의 시험 문항(`crates/store/fixtures/problems/`).
const EXAM_PROBLEM: &str = "count-vowels";
use promptlab_worker::Outcome;
use serde_json::{Value, json};

const TIME_LIMIT: i64 = 600;

/// 받은 메일을 남기는 가짜 발송기. `fail`이면 실패한다.
#[derive(Default)]
struct FakeMailer {
    sent: Mutex<Vec<Mail>>,
    fail: bool,
}

impl Mailer for FakeMailer {
    fn send<'a>(&'a self, mail: &'a Mail) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async move {
            if self.fail {
                return Err(MailError::Failed("연결 거부".into()));
            }
            self.sent.lock().unwrap().push(mail.clone());
            Ok(())
        })
    }
}

struct Env {
    app: axum::Router,
    clock: Arc<FixedClock>,
    mailer: Arc<FakeMailer>,
    pool: PgPool,
}

fn env_with(pool: PgPool, mailer: FakeMailer) -> Env {
    let clock = FixedClock::at(NOW);
    let mailer = Arc::new(mailer);
    Env {
        app: app_parts(pool.clone(), clock.clone(), mailer.clone(), None),
        clock,
        mailer,
        pool,
    }
}

fn env(pool: PgPool) -> Env {
    env_with(pool, FakeMailer::default())
}

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
) -> StatusCode {
    call(app, method, uri, Some(&token_for(sub)), body).await.0
}

/// ALICE(관리자)의 조직에 BOB 출제자, CAROL 감독관, DAVE 열람자. ERIN은 바깥 사람(응시자).
async fn staffed_org(app: &axum::Router) -> OrgView {
    let org: OrgView = ok(
        app,
        "POST",
        "/orgs",
        ALICE,
        Some(json!({"name": "에이 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    common::approve_org(app, &org.id).await;
    for (sub, role) in [
        (BOB, Role::Author),
        (CAROL, Role::Proctor),
        (DAVE, Role::Viewer),
    ] {
        let invite: InviteCreated = ok(
            app,
            "POST",
            &format!("/orgs/{}/invites", org.id),
            ALICE,
            Some(json!({"role": role})),
            StatusCode::CREATED,
        )
        .await;
        let _: OrgView = ok(
            app,
            "POST",
            &format!("/invites/{}/accept", invite.token),
            sub,
            Some(json!({"terms_version": promptlab_shared::terms::TERMS_VERSION})),
            StatusCode::OK,
        )
        .await;
    }
    org
}

fn exam_body(purpose: Purpose, reveal: bool) -> Value {
    json!({
        "title": "코딩 역량 1차",
        "purpose": purpose,
        "reveal_scores": reveal,
        "items": [
            {"problem_id": EXAM_PROBLEM, "limits": {"time_limit_secs": TIME_LIMIT}},
            {"problem_id": EXAM_PROBLEM, "limits": {"time_limit_secs": 1200}},
        ],
        "settings": {
            "score_weight": 70,
            "rubric": [{"name": "검증 습관", "description": "출력을 예시로 확인한다"}],
        },
    })
}

async fn create_exam(
    app: &axum::Router,
    org: &OrgView,
    by: &str,
    purpose: Purpose,
    reveal: bool,
) -> ExamView {
    ok(
        app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        by,
        Some(exam_body(purpose, reveal)),
        StatusCode::CREATED,
    )
    .await
}

async fn invite(
    app: &axum::Router,
    org: &OrgView,
    exam: &ExamView,
    emails: &[&str],
) -> ExamInvitesCreated {
    ok(
        app,
        "POST",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        BOB,
        Some(json!({"emails": emails})),
        StatusCode::CREATED,
    )
    .await
}

/// 시험을 만들고 ERIN을 초대해 배정까지.
async fn assigned(e: &Env, purpose: Purpose, reveal: bool) -> (OrgView, ExamView, AssignmentView) {
    assigned_with(e, purpose, reveal, None).await
}

/// `languages`를 주면 시험 설정의 허용 언어로 만든다.
async fn assigned_with(
    e: &Env,
    purpose: Purpose,
    reveal: bool,
    languages: Option<Value>,
) -> (OrgView, ExamView, AssignmentView) {
    let org = staffed_org(&e.app).await;
    let mut body = exam_body(purpose, reveal);
    if let Some(l) = languages {
        body["settings"]["allowed_languages"] = l;
    }
    let exam: ExamView = ok(
        &e.app,
        "POST",
        &format!("/orgs/{}/exams", org.id),
        BOB,
        Some(body),
        StatusCode::CREATED,
    )
    .await;
    let created = invite(&e.app, &org, &exam, &["erin@example.com"]).await;
    let token = created.invites[0].token.clone().unwrap();
    let assignment: AssignmentView = ok(
        &e.app,
        "POST",
        &format!("/exam-invites/{token}/accept"),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    // 웹캠 안내 동의·기준 사진(#13) — 이것 없이는 문항을 시작하지 못한다(webcam_api.rs).
    webcam_ready(&e.app, &token_for(ERIN), &assignment.id).await;
    let mine: Vec<AssignmentView> =
        ok(&e.app, "GET", "/me/assignments", ERIN, None, StatusCode::OK).await;
    let assignment = mine.into_iter().find(|a| a.id == assignment.id).unwrap();
    (org, exam, assignment)
}

/// 내장 문제를 모두 맞히는 가짜 판정기용 코드(`common::judge`).
const CODE: &str = "def correct(): pass";

async fn invite_statuses(e: &Env, org: &OrgView, exam: &ExamView) -> Vec<InviteStatus> {
    let list: Vec<ExamInviteView> = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        CAROL,
        None,
        StatusCode::OK,
    )
    .await;
    list.into_iter().map(|i| i.status).collect()
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 출제자가_문제은행에서_시험을_만들고_구성원이_본다(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;

    // 문제은행은 공개 보기만(숨은 입력 없음 — call()이 건다).
    let bank: Vec<PublicCodingProblem> = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/problem-bank", org.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(bank.iter().any(|p| p.id == EXAM_PROBLEM));

    let exam = create_exam(&e.app, &org, BOB, Purpose::Recruiting, true).await;
    assert_eq!(exam.items.len(), 2);
    assert_eq!(exam.settings.score_weight, 70);
    assert_eq!(exam.settings.rubric.len(), 1);
    assert!(!exam.reveal_scores, "채용 시험은 공개 설정을 받지 않는다");

    for sub in [ALICE, BOB, CAROL, DAVE] {
        let list: Vec<ExamView> = ok(
            &e.app,
            "GET",
            &format!("/orgs/{}/exams", org.id),
            sub,
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(list, vec![exam.clone()], "{sub}");
    }
    let one: ExamView = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}", org.id, exam.id),
        DAVE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(one, exam);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 시험_관리_권한이_없으면_403_구성원이_아니면_404(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exams = format!("/orgs/{}/exams", org.id);
    let bank = format!("/orgs/{}/problem-bank", org.id);
    for sub in [CAROL, DAVE] {
        assert_eq!(
            status_of(
                &e.app,
                "POST",
                &exams,
                sub,
                Some(exam_body(Purpose::Internal, false))
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(&e.app, "GET", &bank, sub, None).await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &exams,
            ERIN,
            Some(exam_body(Purpose::Internal, false))
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_of(&e.app, "GET", &exams, ERIN, None).await,
        StatusCode::NOT_FOUND
    );

    // 다른 조직의 시험은 우리 조직 경로로 보이지 않는다.
    let exam = create_exam(&e.app, &org, ALICE, Purpose::Internal, false).await;
    let other: OrgView = ok(
        &e.app,
        "POST",
        "/orgs",
        ERIN,
        Some(json!({"name": "비 회사", "terms_version": promptlab_shared::terms::TERMS_VERSION})),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(
        status_of(
            &e.app,
            "GET",
            &format!("/orgs/{}/exams/{}", other.id, exam.id),
            ERIN,
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
    // 출제자는 초대를 내고 현황을 본다. 열람자도 현황은 본다(결과 보기), 초대는 못 낸다.
    let invites = format!("/orgs/{}/exams/{}/invites", org.id, exam.id);
    assert_eq!(
        status_of(&e.app, "GET", &invites, DAVE, None).await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &invites,
            DAVE,
            Some(json!({"emails": ["x@example.com"]}))
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 문항과_제한이_틀리면_422(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exams = format!("/orgs/{}/exams", org.id);
    let item = |problem: &str, secs: u32| json!({"problem_id": problem, "limits": {"time_limit_secs": secs}});
    for (title, items) in [
        ("빈 시험", json!([])),
        ("없는 문제", json!([item("없는-문제", 600)])),
        ("연습 문제", json!([item("array-sum", 600)])),
        ("너무 짧은 시간", json!([item(EXAM_PROBLEM, 10)])),
        ("   ", json!([item(EXAM_PROBLEM, 600)])),
    ] {
        let body = json!({"title": title, "purpose": "internal", "items": items});
        assert_eq!(
            status_of(&e.app, "POST", &exams, BOB, Some(body)).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{title}"
        );
    }
    // 채점 설정 범위: w 0~100, 추가 기준표 최대 2개·이름 30자·설명 300자.
    let rubric = |name: &str, description: &str| json!({"name": name, "description": description});
    for (title, settings) in [
        ("w 101", json!({"score_weight": 101})),
        (
            "기준표 3개",
            json!({"score_weight": 50,
            "rubric": [rubric("a", "b"), rubric("c", "d"), rubric("e", "f")]}),
        ),
        (
            "기준표 이름 31자",
            json!({"score_weight": 50,
            "rubric": [rubric(&"가".repeat(31), "b")]}),
        ),
        (
            "기준표 설명 301자",
            json!({"score_weight": 50,
            "rubric": [rubric("a", &"가".repeat(301))]}),
        ),
        (
            "허용 언어 없음",
            json!({"score_weight": 50, "allowed_languages": []}),
        ),
        (
            "모르는 언어",
            json!({"score_weight": 50, "allowed_languages": ["python", "rust"]}),
        ),
        (
            "겹친 언어",
            json!({"score_weight": 50, "allowed_languages": ["java", "java"]}),
        ),
        (
            "기준표 빈 이름",
            json!({"score_weight": 50,
            "rubric": [rubric(" ", "b")]}),
        ),
    ] {
        let body = json!({"title": title, "purpose": "internal", "items": [item(EXAM_PROBLEM, 600)], "settings": settings});
        assert_eq!(
            status_of(&e.app, "POST", &exams, BOB, Some(body)).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{title}"
        );
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 일괄_초대는_메일을_보내고_틀린_주소를_돌려준다(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exam = create_exam(&e.app, &org, BOB, Purpose::Recruiting, false).await;
    let created = invite(
        &e.app,
        &org,
        &exam,
        &[
            "Erin@Example.com",
            "frank@example.com, 틀린주소",
            "erin@example.com",
        ],
    )
    .await;
    assert_eq!(created.rejected, vec!["틀린주소".to_owned()]);
    let emails: Vec<&str> = created.invites.iter().map(|i| i.email.as_str()).collect();
    assert_eq!(emails, ["erin@example.com", "frank@example.com"]);
    assert!(
        created
            .invites
            .iter()
            .all(|i| i.delivery == Delivery::Sent && i.token.is_some())
    );

    let sent = e.mailer.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    let token = created.invites[0].token.as_deref().unwrap();
    assert_eq!(sent[0].to, "erin@example.com");
    assert!(
        sent[0]
            .body
            .contains(&format!("{PUBLIC_URL}/exam-invite/{token}")),
        "{}",
        sent[0].body
    );
    assert!(sent[0].subject.contains(&exam.title));

    let list: Vec<ExamInviteView> = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
        BOB,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(list.len(), 2);
    assert!(
        list.iter().all(|i| i.status == InviteStatus::Invited
            && !i.accepted
            && i.delivery == Delivery::Sent)
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 메일이_실패하거나_꺼져도_초대는_만들어지고_링크로_배정된다(
    pool: PgPool,
) {
    for mailer in [
        FakeMailer {
            fail: true,
            ..Default::default()
        },
        FakeMailer::default(),
    ] {
        let failing = mailer.fail;
        let pool = pool.clone();
        // 실패하는 발송기와, 설정이 없는(Disabled) 서버.
        let app = if failing {
            env_with(pool.clone(), mailer).app
        } else {
            app_parts(
                pool.clone(),
                FixedClock::at(NOW),
                Arc::new(promptlab_server::mail::Disabled),
                None,
            )
        };
        sqlx::query("TRUNCATE orgs, users, exam_invites CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        let org = staffed_org(&app).await;
        let exam = create_exam(&app, &org, BOB, Purpose::Recruiting, false).await;
        let created = invite(&app, &org, &exam, &["erin@example.com"]).await;
        let want = if failing {
            Delivery::Failed
        } else {
            Delivery::NotConfigured
        };
        assert_eq!(created.invites[0].delivery, want);
        let token = created.invites[0]
            .token
            .clone()
            .expect("링크 복사로 쓸 토큰");
        let assignment: AssignmentView = ok(
            &app,
            "POST",
            &format!("/exam-invites/{token}/accept"),
            ERIN,
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(assignment.exam_id, exam.id);
        let list: Vec<ExamInviteView> = ok(
            &app,
            "GET",
            &format!("/orgs/{}/exams/{}/invites", org.id, exam.id),
            BOB,
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(list[0].delivery, want);
        assert!(list[0].accepted);
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 다시_초대하면_옛_링크는_죽고_배정이_끝난_사람에게는_링크를_다시_내지_않는다(
    pool: PgPool,
) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exam = create_exam(&e.app, &org, BOB, Purpose::Recruiting, false).await;
    let first = invite(&e.app, &org, &exam, &["erin@example.com"])
        .await
        .invites[0]
        .token
        .clone()
        .unwrap();
    let second = invite(&e.app, &org, &exam, &["erin@example.com"])
        .await
        .invites[0]
        .token
        .clone()
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("/exam-invites/{first}/accept"),
            ERIN,
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
    let _: AssignmentView = ok(
        &e.app,
        "POST",
        &format!("/exam-invites/{second}/accept"),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    // 같은 사람이 다시 눌러도 같은 배정.
    let again: AssignmentView = ok(
        &e.app,
        "POST",
        &format!("/exam-invites/{second}/accept"),
        ERIN,
        None,
        StatusCode::OK,
    )
    .await;
    // 다른 사람은 그 링크로 받지 못한다.
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("/exam-invites/{second}/accept"),
            DAVE,
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
    let third = invite(&e.app, &org, &exam, &["erin@example.com"]).await;
    assert!(third.invites[0].already_assigned && third.invites[0].token.is_none());
    assert_eq!(third.invites[0].id, again.id);
    assert_eq!(
        e.mailer.sent.lock().unwrap().len(),
        2,
        "배정이 끝난 사람에게는 다시 보내지 않는다"
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 만료된_초대는_받지_못한다(pool: PgPool) {
    let e = env(pool);
    let org = staffed_org(&e.app).await;
    let exam = create_exam(&e.app, &org, BOB, Purpose::Internal, false).await;
    let token = invite(&e.app, &org, &exam, &["erin@example.com"])
        .await
        .invites[0]
        .token
        .clone()
        .unwrap();
    e.clock
        .advance(promptlab_shared::exam::EXAM_INVITE_SECS + 1);
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("/exam-invites/{token}/accept"),
            ERIN,
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 초대_배정_응시_제출_채점_흐름과_현황(pool: PgPool) {
    let e = env(pool);
    let (org, exam, assignment) = assigned(&e, Purpose::Recruiting, false).await;
    assert_eq!(assignment.org_name, "에이 회사");
    assert!(!assignment.scores_visible);
    assert!(
        assignment
            .items
            .iter()
            .all(|i| i.state == ItemState::NotStarted)
    );
    // 응시자는 조직 구성원이 되지 않는다.
    assert_eq!(
        status_of(&e.app, "GET", &format!("/orgs/{}", org.id), ERIN, None).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        invite_statuses(&e, &org, &exam).await,
        [InviteStatus::Invited]
    );

    let mine: Vec<AssignmentView> =
        ok(&e.app, "GET", "/me/assignments", ERIN, None, StatusCode::OK).await;
    assert_eq!(mine, vec![assignment.clone()]);
    let others: Vec<AssignmentView> =
        ok(&e.app, "GET", "/me/assignments", DAVE, None, StatusCode::OK).await;
    assert!(others.is_empty());

    // 문항 0 시작: 문제(공개 보기)와 이 문항의 제한·마감.
    let base = format!("/assignments/{}/items", assignment.id);
    let started: ItemStarted = ok(
        &e.app,
        "POST",
        &format!("{base}/0/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(started.problem.id, EXAM_PROBLEM);
    assert_eq!(started.limits.time_limit_secs, TIME_LIMIT as u32);
    assert_eq!(started.problem.templates.len(), 4, "4개 언어 틀");
    assert_eq!(started.deadline, NOW + TIME_LIMIT);
    assert_eq!(
        invite_statuses(&e, &org, &exam).await,
        [InviteStatus::InProgress]
    );

    // 다시 시작해도 처음 시각 그대로.
    e.clock.advance(100);
    let again: ItemStarted = ok(
        &e.app,
        "POST",
        &format!("{base}/0/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(again.started_at, NOW);
    assert_eq!(again.now, NOW + 100);

    let view: SubmissionView = ok(
        &e.app,
        "POST",
        &format!("{base}/0/submit"),
        ERIN,
        Some(logged_submit_body(&e.app, &format!("{base}/0/submit"), &token_for(ERIN), CODE).await),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(
        view.state,
        SubmissionState::Completed,
        "채용 시험 응시자에게는 응시 완료만"
    );
    assert_eq!(view.org_id.as_deref(), Some(org.id.as_str()));
    // 두 번 낼 수 없다.
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("{base}/0/submit"),
            ERIN,
            Some(
                logged_submit_body(&e.app, &format!("{base}/0/submit"), &token_for(ERIN), CODE)
                    .await
            )
        )
        .await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("{base}/0/start"),
            ERIN,
            Some(start_body())
        )
        .await,
        StatusCode::CONFLICT
    );

    let _: ItemStarted = ok(
        &e.app,
        "POST",
        &format!("{base}/1/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    let view1: SubmissionView = ok(
        &e.app,
        "POST",
        &format!("{base}/1/submit"),
        ERIN,
        Some(logged_submit_body(&e.app, &format!("{base}/1/submit"), &token_for(ERIN), CODE).await),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(
        invite_statuses(&e, &org, &exam).await,
        [InviteStatus::Submitted]
    );

    let mine: Vec<AssignmentView> =
        ok(&e.app, "GET", "/me/assignments", ERIN, None, StatusCode::OK).await;
    assert_eq!(
        mine[0].items[0].state,
        ItemState::Submitted {
            submission_id: view.id.clone()
        }
    );

    for _ in 0..2 {
        assert!(matches!(grade(&e.pool).await, Outcome::Graded(_)));
    }
    assert_eq!(
        invite_statuses(&e, &org, &exam).await,
        [InviteStatus::Graded]
    );

    // 응시자 본인: 채점이 끝나도 「응시 완료」만. 고객사(결과 보기)는 점수·지표를 본다. 출제자는 못 본다.
    for id in [&view.id, &view1.id] {
        let path = format!("/submissions/{id}");
        let seen: SubmissionView = ok(&e.app, "GET", &path, ERIN, None, StatusCode::OK).await;
        assert_eq!(seen.state, SubmissionState::Completed);
        for sub in [ALICE, CAROL, DAVE] {
            let full: SubmissionView = ok(&e.app, "GET", &path, sub, None, StatusCode::OK).await;
            let SubmissionState::Done { report } = &full.state else {
                panic!("{sub}: {:?}", full.state);
            };
            assert_eq!(report.passed, report.total, "{sub}");
            assert_eq!(report.language, promptlab_shared::coding::Language::Python);
        }
        assert_eq!(
            status_of(&e.app, "GET", &path, BOB, None).await,
            StatusCode::NOT_FOUND
        );
    }
    let org_list: Vec<SubmissionView> = ok(
        &e.app,
        "GET",
        &format!("/orgs/{}/submissions", org.id),
        DAVE,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(org_list.len(), 2);
    // 소요 시간은 서버가 잰다(시작부터 제출까지 100초).
    let elapsed: i32 =
        sqlx::query_scalar("SELECT elapsed_secs FROM submissions WHERE id = $1::uuid")
            .bind(&view.id)
            .fetch_one(&e.pool)
            .await
            .unwrap();
    assert_eq!(elapsed, 100);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 사내_진단은_공개_설정일_때만_응시자가_점수를_본다(pool: PgPool) {
    for reveal in [true, false] {
        sqlx::query("TRUNCATE orgs, users, exam_invites, submissions CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        let e = env(pool.clone());
        let (_, exam, assignment) = assigned(&e, Purpose::Internal, reveal).await;
        assert_eq!(exam.reveal_scores, reveal);
        assert_eq!(assignment.scores_visible, reveal);
        let base = format!("/assignments/{}/items", assignment.id);
        let _: ItemStarted = ok(
            &e.app,
            "POST",
            &format!("{base}/0/start"),
            ERIN,
            Some(start_body()),
            StatusCode::OK,
        )
        .await;
        let view: SubmissionView = ok(
            &e.app,
            "POST",
            &format!("{base}/0/submit"),
            ERIN,
            Some(
                logged_submit_body(&e.app, &format!("{base}/0/submit"), &token_for(ERIN), CODE)
                    .await,
            ),
            StatusCode::ACCEPTED,
        )
        .await;
        grade(&e.pool).await;
        let seen: SubmissionView = ok(
            &e.app,
            "GET",
            &format!("/submissions/{}", view.id),
            ERIN,
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            matches!(seen.state, SubmissionState::Done { .. }),
            reveal,
            "{:?}",
            seen.state
        );
    }
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 서버가_제한_시간을_강제한다(pool: PgPool) {
    let e = env(pool);
    let (_, _, assignment) = assigned(&e, Purpose::Recruiting, false).await;
    let base = format!("/assignments/{}/items", assignment.id);

    // 시작하지 않은 문항은 낼 수 없다.
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("{base}/0/submit"),
            ERIN,
            Some(
                logged_submit_body(&e.app, &format!("{base}/0/submit"), &token_for(ERIN), CODE)
                    .await
            )
        )
        .await,
        StatusCode::CONFLICT
    );
    let _: ItemStarted = ok(
        &e.app,
        "POST",
        &format!("{base}/0/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    // 마감 + 여유까지는 받는다(문항 1), 넘으면 409(문항 0).
    let _: ItemStarted = ok(
        &e.app,
        "POST",
        &format!("{base}/1/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    e.clock.advance(TIME_LIMIT + SUBMIT_GRACE_SECS + 1);
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            &format!("{base}/0/submit"),
            ERIN,
            Some(
                logged_submit_body(&e.app, &format!("{base}/0/submit"), &token_for(ERIN), CODE)
                    .await
            )
        )
        .await,
        StatusCode::CONFLICT
    );
    let _: SubmissionView = ok(
        &e.app,
        "POST",
        &format!("{base}/1/submit"),
        ERIN,
        Some(logged_submit_body(&e.app, &format!("{base}/1/submit"), &token_for(ERIN), CODE).await),
        StatusCode::ACCEPTED,
    )
    .await;
}

/// 시험 제출은 과정 로그가 정본이다: `log_seq` 안의 마지막 `Submitted` 사건(언어·코드 sha256)과 본문이 같아야 받는다.
#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 시험_제출은_과정_로그의_제출_사건과_맞아야_한다(pool: PgPool) {
    use promptlab_shared::coding::{Language, MAX_CODE_BYTES};
    use promptlab_shared::log::{LogEvent, code_sha256};
    let e = env(pool);
    let (org, exam, assignment) = assigned(&e, Purpose::Recruiting, false).await;
    let item = format!("/assignments/{}/items/0", assignment.id);
    let submit = format!("{item}/submit");
    let _: ItemStarted = ok(
        &e.app,
        "POST",
        &format!("{item}/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    let post = |body: Value| {
        let app = e.app.clone();
        let submit = submit.clone();
        async move {
            call(&app, "POST", &submit, Some(&token_for(ERIN)), Some(body))
                .await
                .0
        }
    };

    // 제출 사건이 없는 로그(언어 고름·스냅숏만)면 422.
    let seq = upload_events(
        &e.app,
        &item,
        &token_for(ERIN),
        vec![
            LogEvent::LanguageChosen {
                language: Language::Python,
            },
            LogEvent::CodeSnapshot { code: CODE.into() },
        ],
    )
    .await;
    assert_eq!(seq, 2);
    assert_eq!(
        post(json!({"language": "python", "code": CODE, "log_seq": seq})).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // 제출 사건을 올린다(언어 python·sha256(CODE)).
    let seq = upload_events(
        &e.app,
        &item,
        &token_for(ERIN),
        vec![LogEvent::Submitted {
            language: Language::Python,
            code_sha256: code_sha256(CODE),
        }],
    )
    .await;
    assert_eq!(seq, 3);
    // 로그가 덜 올라왔다(앱은 사건 4개를 썼다고 한다) → 409.
    assert_eq!(
        post(json!({"language": "python", "code": CODE, "log_seq": seq + 1})).await,
        StatusCode::CONFLICT
    );
    // 코드가 다르거나 언어가 다르면 422.
    assert_eq!(
        post(json!({"language": "python", "code": format!("{CODE} "), "log_seq": seq})).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        post(json!({"language": "java", "code": CODE, "log_seq": seq})).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // 빈 코드·64KB 초과는 422.
    assert_eq!(
        post(json!({"language": "python", "code": "", "log_seq": seq})).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        post(json!({"language": "python", "code": "x".repeat(MAX_CODE_BYTES + 1), "log_seq": seq}))
            .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // `log_seq` 밖(뒤)의 제출 사건은 보지 않는다: 앞 2개만 본다고 하면 제출 사건이 없다 → 422.
    assert_eq!(
        post(json!({"language": "python", "code": CODE, "log_seq": 2})).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // 맞으면 받는다 → 채점 → 결과에 언어·통과 수.
    assert_eq!(
        post(json!({"language": "python", "code": CODE, "log_seq": seq})).await,
        StatusCode::ACCEPTED
    );
    assert!(matches!(grade(&e.pool).await, Outcome::Graded(_)));
    let (status, results) = call(
        &e.app,
        "GET",
        &format!("/orgs/{}/exams/{}/results", org.id, exam.id),
        Some(&token_for(ALICE)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{results}");
    let first = &results["rows"][0]["items"][0];
    let total = promptlab_store::catalog::find(EXAM_PROBLEM)
        .unwrap()
        .hidden
        .len();
    assert_eq!(first["language"], "python", "{results}");
    assert_eq!(first["passed"], total, "{results}");
    assert_eq!(first["total"], total, "{results}");
}

/// 마지막 제출 사건이 정본이다 — 앱에서 두 번 「제출」을 눌렀다면 나중 것과 맞아야 한다.
#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 마지막_제출_사건과_맞아야_한다(pool: PgPool) {
    use promptlab_shared::coding::Language;
    use promptlab_shared::log::{LogEvent, code_sha256};
    let e = env(pool);
    let (_, _, assignment) = assigned(&e, Purpose::Recruiting, false).await;
    let item = format!("/assignments/{}/items/0", assignment.id);
    let _: ItemStarted = ok(
        &e.app,
        "POST",
        &format!("{item}/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    let seq = upload_events(
        &e.app,
        &item,
        &token_for(ERIN),
        vec![
            LogEvent::Submitted {
                language: Language::Python,
                code_sha256: code_sha256("first-only"),
            },
            LogEvent::Submitted {
                language: Language::Python,
                code_sha256: code_sha256(CODE),
            },
        ],
    )
    .await;
    let submit = format!("{item}/submit");
    let (status, _) = call(
        &e.app,
        "POST",
        &submit,
        Some(&token_for(ERIN)),
        Some(json!({"language": "python", "code": "first-only", "log_seq": seq})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(
        &e.app,
        "POST",
        &submit,
        Some(&token_for(ERIN)),
        Some(json!({"language": "python", "code": CODE, "log_seq": seq})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 남의_배정과_없는_문항은_404(pool: PgPool) {
    let e = env(pool);
    let (_, _, assignment) = assigned(&e, Purpose::Recruiting, false).await;
    let base = format!("/assignments/{}/items", assignment.id);
    for sub in [ALICE, DAVE] {
        assert_eq!(
            status_of(
                &e.app,
                "POST",
                &format!("{base}/0/start"),
                sub,
                Some(start_body())
            )
            .await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(
                &e.app,
                "POST",
                &format!("{base}/0/submit"),
                sub,
                Some(
                    logged_submit_body(&e.app, &format!("{base}/0/submit"), &token_for(sub), CODE)
                        .await
                )
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }
    for item in ["2", "-1", "x"] {
        assert_eq!(
            status_of(
                &e.app,
                "POST",
                &format!("{base}/{item}/start"),
                ERIN,
                Some(start_body())
            )
            .await,
            StatusCode::NOT_FOUND,
            "{item}"
        );
    }
    assert_eq!(
        status_of(
            &e.app,
            "POST",
            "/assignments/not-a-uuid/items/0/start",
            ERIN,
            Some(start_body())
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

/// 대시보드 산출물 폴더를 주면 API에 없는 경로를 그 폴더에서(없는 파일은 index.html) 서빙한다.
#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 대시보드_정적_파일을_서빙하고_api_경로는_그대로다(pool: PgPool) {
    let dir = std::env::temp_dir().join(format!("promptlab-dash-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><div id=\"main\"></div>",
    )
    .unwrap();
    std::fs::write(dir.join("promptlab-dashboard.js"), "export default 1;").unwrap();
    let app = app_parts(
        pool,
        FixedClock::at(NOW),
        Arc::new(promptlab_server::mail::Disabled),
        Some(dir.clone()),
    );

    let get = |uri: &str| {
        let app = app.clone();
        let uri = uri.to_owned();
        async move {
            let (status, headers, body) =
                send(&app, Request::builder().method("GET").uri(uri), None).await;
            (status, headers, body)
        }
    };
    for path in ["/", "/exam-invite/abc", "/invite/xyz"] {
        let (status, headers, _) = get(path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let ct = headers[header::CONTENT_TYPE].to_str().unwrap();
        assert!(ct.starts_with("text/html"), "{path}: {ct}");
        assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    }
    let (status, headers, _) = get("/promptlab-dashboard.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    // API는 그대로: /health JSON, /api 아래 없는 경로는 JSON 404, 토큰 없는 API는 401.
    let (status, _, body) = get("/health").await;
    assert_eq!(
        (status, body["status"].as_str()),
        (StatusCode::OK, Some("ok"))
    );
    let (status, _, body) = get("/api/health").await;
    assert_eq!(
        (status, body["status"].as_str()),
        (StatusCode::OK, Some("ok"))
    );
    let (status, _, body) = get("/api/없는-경로").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].is_string(), "{body}");
    assert_eq!(get("/me").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(get("/api/me/assignments").await.0, StatusCode::UNAUTHORIZED);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 허용 언어(#96): 기본은 4개 전부, 시험 설정으로 좁히면 시작 응답에 허용 언어·그 틀만 실리고 밖의 언어 제출은 422.
#[sqlx::test(migrator = "promptlab_store::MIGRATOR")]
async fn 허용_언어_밖의_제출은_422이고_시작_응답은_허용_언어만_싣는다(
    pool: PgPool,
) {
    use promptlab_shared::coding::Language;
    use promptlab_shared::log::{LogEvent, code_sha256};
    // 기본(설정 없음) = 4개 전부.
    let e = env(pool.clone());
    let (_, exam, assignment) = assigned(&e, Purpose::Recruiting, false).await;
    assert_eq!(exam.settings.allowed_languages, Language::ALL.to_vec());
    assert_eq!(assignment.allowed_languages, Language::ALL.to_vec());

    // 파이썬·자바스크립트만 허용.
    let e = env(pool);
    let (_, exam, assignment) = assigned_with(
        &e,
        Purpose::Recruiting,
        false,
        Some(json!(["python", "javascript"])),
    )
    .await;
    let allowed = vec![Language::Python, Language::Javascript];
    assert_eq!(exam.settings.allowed_languages, allowed);
    assert_eq!(assignment.allowed_languages, allowed);
    let item = format!("/assignments/{}/items/0", assignment.id);
    let started: ItemStarted = ok(
        &e.app,
        "POST",
        &format!("{item}/start"),
        ERIN,
        Some(start_body()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(started.allowed_languages, allowed);
    assert_eq!(
        started
            .problem
            .templates
            .iter()
            .map(|t| t.language)
            .collect::<Vec<_>>(),
        allowed
    );
    // 허용 밖(java)은 로그 제출 사건이 맞아도 422.
    let seq = upload_events(
        &e.app,
        &item,
        &token_for(ERIN),
        vec![LogEvent::Submitted {
            language: Language::Java,
            code_sha256: code_sha256(CODE),
        }],
    )
    .await;
    let (status, body) = call(
        &e.app,
        "POST",
        &format!("{item}/submit"),
        Some(&token_for(ERIN)),
        Some(json!({"language": "java", "code": CODE, "log_seq": seq})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("java"), "{body}");
    // 허용 언어는 받는다.
    let seq = upload_events(
        &e.app,
        &item,
        &token_for(ERIN),
        vec![LogEvent::Submitted {
            language: Language::Python,
            code_sha256: code_sha256(CODE),
        }],
    )
    .await;
    let (status, body) = call(
        &e.app,
        "POST",
        &format!("{item}/submit"),
        Some(&token_for(ERIN)),
        Some(json!({"language": "python", "code": CODE, "log_seq": seq})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}
