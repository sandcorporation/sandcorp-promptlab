//! 문제은행 문항 시험(#138 — 문항은 실행할 때 디렉터리에서 싣는다, `promptlab_store::catalog`).
//!
//! 대상 디렉터리는 `PROMPTLAB_PROBLEMS_DIR`(운영 문항 — 비공개 데이터 저장소), 없으면 공개 픽스처
//! `crates/store/fixtures/problems/`. 스모크 전용 문항(`crates/store/fixtures/smoke/`)은 늘 함께 본다.
//!
//! - 형식 시험(항상 돈다): `index.json`의 연습·시험 목록과 `*.json` 파일이 맞고, 모두 `CodingProblem`으로 읽히고
//!   `validate()`를 통과한다.
//! - docker 시험(`#[ignore]`): 문항마다 모범 답안을 판정기로 예제 + 히든 전부 돌려 모두 통과해야 한다. 출력은 문항 id와
//!   통과 수·실패 케이스 번호뿐이다(히든 입력·기대값은 찍지 않는다).
//!
//! ```text
//! bash scripts/build-runners.sh --local
//! PROMPTLAB_PROBLEMS_DIR=<운영 문항 디렉터리> cargo test -p promptlab-judge --test bank -- --ignored --test-threads=1
//! ```

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use promptlab_judge::{Judge, RunRequest, Status, passed};
use promptlab_shared::coding::{CodingProblem, Language};
use serde_json::Value;

/// 문제 디렉터리 — `PROMPTLAB_PROBLEMS_DIR`, 없으면 공개 픽스처.
fn dir() -> PathBuf {
    std::env::var_os("PROMPTLAB_PROBLEMS_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || {
                PathBuf::from(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../store/fixtures/problems"
                ))
            },
            PathBuf::from,
        )
}

fn smoke_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../store/fixtures/smoke"
    ))
}

fn json_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter(|p| p.file_name().is_some_and(|n| n != "index.json"))
        .collect();
    files.sort();
    files
}

fn load() -> Vec<CodingProblem> {
    json_files(&dir())
        .into_iter()
        .chain(json_files(&smoke_dir()))
        .map(|p| {
            let text = fs::read_to_string(&p).unwrap();
            let problem: CodingProblem =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let stem = p.file_stem().unwrap().to_string_lossy();
            assert_eq!(problem.id, stem, "파일 이름 = id");
            problem
        })
        .collect()
}

/// `index.json`의 (연습, 시험) id.
fn index() -> (Vec<String>, Vec<String>) {
    let text = fs::read_to_string(dir().join("index.json")).expect("index.json");
    let v: Value = serde_json::from_str(&text).expect("index.json 형식");
    let ids = |k: &str| -> Vec<String> {
        v[k].as_array()
            .unwrap_or_else(|| panic!("index.json `{k}`"))
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect()
    };
    (ids("practice"), ids("exam"))
}

#[test]
fn 문항이_형식을_통과하고_목록과_파일이_맞는다() {
    let problems = load();
    let (practice, exam) = index();
    assert!(
        !practice.is_empty() && !exam.is_empty(),
        "연습·시험 문항이 하나 이상"
    );
    let listed: BTreeSet<&str> = practice.iter().chain(&exam).map(String::as_str).collect();
    assert_eq!(
        listed.len(),
        practice.len() + exam.len(),
        "목록 id 중복 없음"
    );
    let files: BTreeSet<String> = json_files(&dir())
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        files,
        listed
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<BTreeSet<_>>(),
        "index.json 목록 = 파일"
    );
    let mut ids: Vec<&str> = problems.iter().map(|p| p.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), problems.len(), "id 중복 없음(스모크 포함)");
    for p in &problems {
        p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.id));
        assert_eq!(p.reference_language, Language::Python, "{}", p.id);
        assert!(
            p.id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{}: id는 소문자-하이픈",
            p.id
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "docker와 로컬 러너 이미지가 필요하다"]
async fn 모범_답안이_예제와_히든을_모두_통과한다() {
    let judge = Judge::new("docker", 2);
    for p in load() {
        let cases: Vec<&promptlab_shared::coding::TestCase> =
            p.examples.iter().chain(&p.hidden).collect();
        let outcome = judge
            .run(&RunRequest {
                language: p.reference_language,
                code: p.reference_solution.clone(),
                signature: p.signature.clone(),
                cases: cases.iter().map(|c| c.args.clone()).collect(),
                case_time_ms: Some(p.limits.case_time_ms),
            })
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", p.id));
        let expected: Vec<Value> = cases.iter().map(|c| c.expected.clone()).collect();
        let ok = passed(p.signature.returns, &expected, &outcome);
        assert_eq!(ok.len(), cases.len(), "{}", p.id);
        let failed: Vec<(usize, Status, Option<String>)> = ok
            .iter()
            .enumerate()
            .filter(|(_, ok)| !**ok)
            .map(|(i, _)| (i, outcome.cases[i].status, outcome.cases[i].message.clone()))
            .collect();
        assert!(
            failed.is_empty(),
            "{}: 통과하지 못한 케이스(0.. = 예제 뒤 히든) {failed:?}, 컴파일 {:?}, stderr {}",
            p.id,
            outcome.compile,
            outcome.stderr
        );
        eprintln!(
            "{}: {}/{} 통과 (최대 {}ms)",
            p.id,
            cases.len(),
            cases.len(),
            outcome.cases.iter().map(|c| c.ms).max().unwrap_or(0)
        );
    }
}
