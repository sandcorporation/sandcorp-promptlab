//! 편집기 밖 로직(#97): 언어별 초안·허용 언어·코드 스냅숏 주기·클립보드(앱 안 버퍼) 판정.

use std::time::{Duration, Instant};

use promptlab_app::editor::{
    Clipboard, EditorError, EditorState, PASTE_BLOCKED_NOTICE, PasteOutcome, SNAPSHOT_EVERY,
    SnapshotClock, template, truncate,
};
use promptlab_shared::coding::{
    DEFAULT_CASE_TIME_MS, Language, Limits, Param, PublicCodingProblem, Signature, TestCase,
    ValueType, templates,
};
use promptlab_shared::log::{LogEvent, MAX_SNAPSHOT_BYTES};
use serde_json::json;

fn problem() -> PublicCodingProblem {
    let signature = Signature {
        name: "add_one".into(),
        params: vec![Param {
            name: "n".into(),
            ty: ValueType::Int,
        }],
        returns: ValueType::Int,
    };
    PublicCodingProblem {
        id: "p-1".into(),
        title: "하나 더하기".into(),
        description: "n에 1을 더하라".into(),
        templates: templates(&signature),
        signature,
        examples: vec![TestCase {
            args: vec![json!(1)],
            expected: json!(2),
        }],
        limits: Limits {
            case_time_ms: DEFAULT_CASE_TIME_MS,
        },
    }
}

fn snapshot(code: &str) -> Option<LogEvent> {
    Some(LogEvent::CodeSnapshot { code: code.into() })
}

#[test]
fn 처음엔_python_틀로_열고_허용_언어만_고른다() {
    let p = problem();
    let mut e = EditorState::new(&p, &[Language::Cpp, Language::Java]);
    // Python이 없으면 Language::ALL 순서의 첫 언어.
    assert_eq!(e.language(), Language::Java);
    assert_eq!(e.allowed(), &[Language::Java, Language::Cpp]);
    assert_eq!(e.code(), template(&p, Language::Java));
    assert_eq!(
        e.opened(),
        LogEvent::LanguageChosen {
            language: Language::Java
        }
    );
    assert_eq!(
        e.choose(Language::Python),
        Err(EditorError::NotAllowed(Language::Python))
    );
    assert_eq!(e.choose(Language::Java), Ok(None));

    let practice = EditorState::new(&p, &[]);
    assert_eq!(practice.language(), Language::Python);
    assert_eq!(practice.allowed(), &Language::ALL);
    assert!(
        practice.code().contains("def add_one"),
        "{}",
        practice.code()
    );
}

#[test]
fn 언어를_바꿔도_앞_언어의_초안이_남는다() {
    let p = problem();
    let mut e = EditorState::new(&p, &[]);
    e.edit("def add_one(n):\n    return n + 1\n".into());
    assert_eq!(
        e.choose(Language::Cpp),
        Ok(Some(LogEvent::LanguageChosen {
            language: Language::Cpp
        }))
    );
    assert_eq!(e.code(), template(&p, Language::Cpp));
    e.choose(Language::Python).unwrap();
    assert_eq!(e.code(), "def add_one(n):\n    return n + 1\n");
}

#[test]
fn 스냅숏은_바뀌었을_때_5초마다_하나다() {
    let t0 = Instant::now();
    let mut clock = SnapshotClock::new();
    assert_eq!(clock.tick(t0, "a"), snapshot("a"));
    // 5초 안에는 바뀌어도 기다린다.
    assert_eq!(clock.tick(t0 + Duration::from_secs(2), "ab"), None);
    assert_eq!(clock.tick(t0 + SNAPSHOT_EVERY, "ab"), snapshot("ab"));
    // 안 바뀌면 남기지 않는다.
    assert_eq!(clock.tick(t0 + SNAPSHOT_EVERY * 3, "ab"), None);
    // 실행·제출 바로 전은 주기와 상관없이(바뀌었으면).
    assert_eq!(clock.force(t0 + SNAPSHOT_EVERY * 3, "abc"), snapshot("abc"));
    assert_eq!(clock.force(t0 + SNAPSHOT_EVERY * 3, "abc"), None);
}

#[test]
fn 스냅숏은_64kb에서_글자_경계로_자른다() {
    let big = "가".repeat(MAX_SNAPSHOT_BYTES); // 3바이트 글자
    let cut = truncate(&big, MAX_SNAPSHOT_BYTES);
    assert!(cut.len() <= MAX_SNAPSHOT_BYTES);
    assert!(cut.len() > MAX_SNAPSHOT_BYTES - 3);
    let mut clock = SnapshotClock::new();
    let Some(LogEvent::CodeSnapshot { code }) = clock.tick(Instant::now(), &big) else {
        panic!("스냅숏이어야 한다");
    };
    assert_eq!(code, cut);
    assert_eq!(truncate("짧다", 100), "짧다");
}

#[test]
fn 밖에서_온_글은_막고_앱_안_복사는_넣는다() {
    let mut clip = Clipboard::new();
    // 버퍼가 비었고 OS 클립보드에 글이 있다 = 밖에서 복사한 글.
    let blocked = clip.paste("print('밖')");
    assert_eq!(blocked, PasteOutcome::Blocked { chars: 10 });
    assert_eq!(blocked.event(), Some(LogEvent::PasteBlocked { chars: 10 }));
    assert!(PASTE_BLOCKED_NOTICE.contains("붙여 넣을 수 없습니다"));
    // 아무것도 없으면 아무 일도 없다.
    assert_eq!(clip.paste(""), PasteOutcome::Nothing);
    assert_eq!(PasteOutcome::Nothing.event(), None);
    // 빈 선택 복사는 무시.
    clip.copied("");
    assert_eq!(clip.paste(""), PasteOutcome::Nothing);
    // 앱 안에서 복사하면 OS 클립보드가 무엇이든 버퍼를 넣는다(밖의 글은 들어오지 않는다).
    clip.copied("return n + 1");
    let inserted = clip.paste("밖의 글");
    assert_eq!(inserted, PasteOutcome::Insert("return n + 1".into()));
    assert_eq!(inserted.event(), None);
}
