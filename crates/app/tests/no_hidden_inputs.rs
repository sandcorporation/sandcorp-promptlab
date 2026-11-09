//! 불변식: 숨은 입력은 앱으로 오지 않는다.
//!
//! - 타입: 앱의 세션·클라이언트는 [`PublicCodingProblem`]과 `GradeReport`만 다룬다(숨은 입력 필드가 없다).
//! - 의존: 숨은 입력이 든 문제 목록은 `promptlab-store`에 있다. 앱은 그것도, 그것에 기대는
//!   서버·워커도 (직접이든 건너서든) 의존하지 않는다 — `cargo tree`의 의존 그래프로 확인한다.

use std::collections::HashSet;
use std::process::Command;

use promptlab_app::session::{Session, SessionEvent};
use promptlab_shared::coding::PublicCodingProblem;

const SERVER_ONLY: &[&str] = &["promptlab-store", "promptlab-server", "promptlab-worker"];

/// `package`가 (dev 의존 빼고) 직접·건너서 기대는 패키지 이름들. 이 기계(host)용 그래프다.
fn dependencies_of(package: &str) -> HashSet<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let output = Command::new(cargo)
        .args([
            "tree",
            "--offline",
            "--manifest-path",
            manifest,
            "-p",
            package,
        ])
        .args(["-e", "no-dev", "--prefix", "none", "-f", "{p}"])
        .output()
        .expect("cargo tree");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn 앱은_숨은_입력이_든_크레이트에_기대지_않는다() {
    // 이 방법이 의존을 잡아내는지 먼저 본다: 서버는 store에 기댄다.
    assert!(dependencies_of("promptlab-server").contains("promptlab-store"));

    let app = dependencies_of("promptlab-app");
    assert!(app.contains("promptlab-shared"));
    for crate_name in SERVER_ONLY {
        assert!(
            !app.contains(*crate_name),
            "응시 앱이 {crate_name} 에 기댄다 — 숨은 입력이 앱으로 갈 수 있다"
        );
    }
}

#[test]
fn 앱이_받는_문제_형식에는_숨은_입력이_없다() {
    // 서버가 숨은 입력을 덧붙여 보내도 앱 타입에는 담기지 않는다.
    let json = serde_json::json!({
        "id": "p", "title": "t", "description": "d",
        "signature": { "name": "f", "params": [{ "name": "s", "type": "string" }], "returns": "string" },
        "examples": [{ "args": ["e"], "expected": "x" }],
        "limits": { "case_time_ms": 2000 },
        "templates": [{ "language": "python", "code": "def f(s):\n    pass\n" }],
        "hidden": [{ "args": ["숨은 입력"], "expected": "x" }],
        "reference_solution": "숨은 입력 모범 답안"
    });
    let problem: PublicCodingProblem = serde_json::from_value(json).unwrap();
    let mut session = Session::new();
    session.apply(SessionEvent::Start).unwrap();
    session
        .apply(SessionEvent::ChecksPassed { problem })
        .unwrap();
    assert!(!format!("{session:?}").contains("숨은 입력"));
}
