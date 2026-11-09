//! 과정 로그 형식: 서명한 체인은 검증되고, 바꾸거나 빼거나 옮겨 붙인 사건은 걸린다.

use promptlab_shared::coding::Language;
use promptlab_shared::log::{
    Attempt, Chain, EditRange, GENESIS, LocalRunKind, LogError, LogEvent, LogSigner, SignedEvent,
    ToolCall, code_sha256, event_hash, open,
};

fn attempt() -> Attempt {
    Attempt {
        assignment_id: "a1".into(),
        position: 0,
    }
}

fn chain_of(signer: &LogSigner, events: Vec<LogEvent>) -> Vec<SignedEvent> {
    let mut prev = GENESIS.to_owned();
    events
        .into_iter()
        .enumerate()
        .map(|(seq, event)| {
            let signed = signer.seal(&attempt(), seq as u64, &prev, 1_000 + seq as i64, event);
            prev = event_hash(&signed.body);
            signed
        })
        .collect()
}

fn sample(signer: &LogSigner) -> Vec<SignedEvent> {
    chain_of(
        signer,
        vec![
            LogEvent::Started,
            LogEvent::LanguageChosen {
                language: Language::Python,
            },
            LogEvent::Submitted {
                language: Language::Python,
                code_sha256: code_sha256("def f(): pass"),
            },
        ],
    )
}

/// 체인 전체를 처음부터 검증한다.
fn verify_all(events: &[SignedEvent], keys: &[String]) -> Result<Chain, LogError> {
    let mut chain = Chain::default();
    for e in events {
        chain.append(&open(e, &attempt(), keys)?)?;
    }
    Ok(chain)
}

#[test]
fn 서명한_체인은_검증되고_머리가_마지막_해시다() {
    let signer = LogSigner::from_seed([7; 32]);
    let events = sample(&signer);
    let chain = verify_all(&events, &[signer.public_key()]).unwrap();
    assert_eq!(chain.next_seq, 3);
    assert_eq!(chain.head, event_hash(&events[2].body));
}

#[test]
fn 원문을_바꾸면_서명이_맞지_않는다() {
    let signer = LogSigner::from_seed([7; 32]);
    let mut events = sample(&signer);
    events[1].body = events[1].body.replace("python", "cpp");
    assert_eq!(
        verify_all(&events, &[signer.public_key()]),
        Err(LogError::BadSignature)
    );
}

#[test]
fn 사건을_빼면_빈틈으로_걸린다() {
    let signer = LogSigner::from_seed([7; 32]);
    let mut events = sample(&signer);
    events.remove(1);
    assert_eq!(
        verify_all(&events, &[signer.public_key()]),
        Err(LogError::Gap {
            expected: 1,
            got: 2
        })
    );
}

#[test]
fn 다시_서명해_끼워_넣어도_체인이_끊긴다() {
    // 키를 가진 사람이 중간 사건을 새로 서명해 바꿔 넣으면 다음 사건의 prev와 어긋난다.
    let signer = LogSigner::from_seed([7; 32]);
    let mut events = sample(&signer);
    let prev = event_hash(&events[0].body);
    events[1] = signer.seal(
        &attempt(),
        1,
        &prev,
        1_001,
        LogEvent::CodeSnapshot {
            code: "바꿔 친 코드".into(),
        },
    );
    assert_eq!(
        verify_all(&events, &[signer.public_key()]),
        Err(LogError::BrokenChain { seq: 2 })
    );
}

#[test]
fn 등록되지_않은_키와_다른_응시의_사건은_받지_않는다() {
    let signer = LogSigner::from_seed([7; 32]);
    let other = LogSigner::from_seed([8; 32]);
    let events = sample(&signer);
    assert_eq!(
        verify_all(&events, &[other.public_key()]),
        Err(LogError::UnknownKey)
    );
    let elsewhere = Attempt {
        assignment_id: "a1".into(),
        position: 1,
    };
    assert_eq!(
        open(&events[0], &elsewhere, &[signer.public_key()]),
        Err(LogError::WrongAttempt)
    );
}

#[test]
fn 모르는_사건_종류도_서명은_검증된다() {
    // 뒤 판의 앱이 더한 사건 종류(#12·#13)를 이 판이 몰라도 체인은 끊기지 않는다.
    let signer = LogSigner::from_seed([7; 32]);
    let first = signer.seal(&attempt(), 0, GENESIS, 1, LogEvent::Started);
    let body = first.body.replace(
        r#"{"type":"started"}"#,
        r#"{"type":"face_missing","secs":3}"#,
    );
    assert_ne!(body, first.body);
    let forged = SignedEvent {
        sig: {
            // 같은 키로 새 원문에 서명한 것처럼 만든다: seal은 종류를 모르니 원문을 직접 서명한다.
            use base64::Engine;
            use ed25519_dalek::{Signer, SigningKey};
            let key = SigningKey::from_bytes(&[7; 32]);
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(key.sign(body.as_bytes()).to_bytes())
        },
        body,
    };
    let opened = open(&forged, &attempt(), &[signer.public_key()]).unwrap();
    assert_eq!(opened.body.event, LogEvent::Unknown);
}

#[test]
fn 서명_키는_디버그_출력에_개인키를_싣지_않는다() {
    let signer = LogSigner::from_seed([9; 32]);
    let shown = format!("{signer:?}");
    assert!(shown.contains(&signer.public_key()));
    assert!(!shown.contains("[9, 9"));
}

#[test]
fn 판_2_사건은_종류마다_원문_형식으로_왕복한다() {
    let cases = [
        (LogEvent::Started, r#"{"type":"started"}"#),
        (
            LogEvent::LanguageChosen {
                language: Language::Cpp,
            },
            r#"{"type":"language_chosen","language":"cpp"}"#,
        ),
        (
            LogEvent::CodeSnapshot {
                code: "x = 1".into(),
            },
            r#"{"type":"code_snapshot","code":"x = 1"}"#,
        ),
        (
            LogEvent::AiMessage {
                text: "힌트".into(),
            },
            r#"{"type":"ai_message","text":"힌트"}"#,
        ),
        (
            LogEvent::AiReply {
                text: "이렇게".into(),
                tool_calls: vec![ToolCall {
                    name: "propose_edit".into(),
                    arguments: serde_json::json!({"line": 3}),
                }],
                stopped: false,
            },
            r#"{"type":"ai_reply","text":"이렇게","tool_calls":[{"name":"propose_edit","arguments":{"line":3}}]}"#,
        ),
        (
            LogEvent::EditProposed {
                proposal: 1,
                range: EditRange {
                    start_line: 1,
                    start_column: 1,
                    end_line: 2,
                    end_column: 5,
                },
                text: "return 0".into(),
            },
            r#"{"type":"edit_proposed","proposal":1,"range":{"start_line":1,"start_column":1,"end_line":2,"end_column":5},"text":"return 0"}"#,
        ),
        (
            LogEvent::EditAccepted { proposal: 1 },
            r#"{"type":"edit_accepted","proposal":1}"#,
        ),
        (
            LogEvent::EditRejected { proposal: 2 },
            r#"{"type":"edit_rejected","proposal":2}"#,
        ),
        (
            LogEvent::LocalRun {
                kind: LocalRunKind::Examples,
                passed: Some(2),
                total: Some(3),
                summary: "예제 2/3 통과".into(),
            },
            r#"{"type":"local_run","kind":"examples","passed":2,"total":3,"summary":"예제 2/3 통과"}"#,
        ),
        (
            LogEvent::LocalRun {
                kind: LocalRunKind::Run,
                passed: None,
                total: None,
                summary: "실행 끝".into(),
            },
            r#"{"type":"local_run","kind":"run","passed":null,"total":null,"summary":"실행 끝"}"#,
        ),
        (
            LogEvent::PasteBlocked { chars: 40 },
            r#"{"type":"paste_blocked","chars":40}"#,
        ),
        (
            LogEvent::Submitted {
                language: Language::Java,
                code_sha256: "ab".into(),
            },
            r#"{"type":"submitted","language":"java","code_sha256":"ab"}"#,
        ),
        (LogEvent::FocusLost, r#"{"type":"focus_lost"}"#),
        (
            LogEvent::FocusReturned { away_ms: 1500 },
            r#"{"type":"focus_returned","away_ms":1500}"#,
        ),
        (
            LogEvent::FullscreenExited,
            r#"{"type":"fullscreen_exited"}"#,
        ),
        (
            LogEvent::NoFace { snapshot: None },
            r#"{"type":"no_face","snapshot":null}"#,
        ),
        (
            LogEvent::MultipleFaces {
                count: 2,
                snapshot: Some("r1".into()),
            },
            r#"{"type":"multiple_faces","count":2,"snapshot":"r1"}"#,
        ),
        (
            LogEvent::GazeAway { snapshot: None },
            r#"{"type":"gaze_away","snapshot":null}"#,
        ),
        (
            LogEvent::Snapshot {
                snapshot: "r2".into(),
            },
            r#"{"type":"snapshot","snapshot":"r2"}"#,
        ),
        (
            LogEvent::CameraUnavailable {
                reason: "분리".into(),
            },
            r#"{"type":"camera_unavailable","reason":"분리"}"#,
        ),
    ];
    for (event, json) in cases {
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            json,
            "{}",
            event.label()
        );
        assert_eq!(serde_json::from_str::<LogEvent>(json).unwrap(), event);
    }
}

#[test]
fn 판_1_대화_사건은_모르는_사건으로_읽히고_판_1_원문은_받지_않는다() {
    for old in [
        r#"{"type":"conversation_turn","conversation":"a","message":"m","reply":"r","tokens":3}"#,
        r#"{"type":"pasted","field":"chat","chars":3}"#,
    ] {
        assert_eq!(
            serde_json::from_str::<LogEvent>(old).unwrap(),
            LogEvent::Unknown
        );
    }
    assert_eq!(promptlab_shared::log::LOG_FORMAT, 2);
}

#[test]
fn 코드_sha256은_소문자_hex다() {
    assert_eq!(
        code_sha256(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}
