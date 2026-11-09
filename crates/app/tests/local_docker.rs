//! 로컬 실행(#97)을 호스트 docker로: 데몬 확인·이미지(운영 digest)·빈 틀·예제 테스트·실행·시간 초과.
//! `#[ignore]` — docker와 러너 이미지가 있어야 한다:
//! `cargo test -p promptlab-app --test local_docker -- --ignored --test-threads=1`
//! (이미지가 없으면 시험이 `pull`로 받는다.)

use promptlab_app::docker_check::{self, DockerCheck};
use promptlab_app::localrun::{DockerError, LocalDocker};
use promptlab_shared::coding::{
    DEFAULT_CASE_TIME_MS, Language, Limits, Param, PublicCodingProblem, Signature, TestCase,
    ValueType, templates,
};
use promptlab_shared::log::LocalRunKind;
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
        examples: vec![
            TestCase {
                args: vec![json!(1)],
                expected: json!(2),
            },
            TestCase {
                args: vec![json!(41)],
                expected: json!(42),
            },
        ],
        limits: Limits {
            case_time_ms: DEFAULT_CASE_TIME_MS,
        },
    }
}

fn docker() -> LocalDocker {
    LocalDocker::detect().expect("docker CLI")
}

#[tokio::test]
#[ignore = "호스트 docker 필요"]
async fn 점검은_네_언어_이미지를_받고_빈_틀을_돌려_준비된다() {
    let d = docker();
    let mut check = DockerCheck::new(&Language::ALL);
    docker_check::drive(Some(&d), &mut check, |_| {}).await;
    assert!(check.ready(), "{}", check.status_text());
}

#[tokio::test]
#[ignore = "호스트 docker 필요"]
async fn 예제_테스트와_실행() {
    let d = docker();
    if !d.has_image(Language::Python).await.unwrap() {
        d.pull(Language::Python, |_| {}).await.unwrap();
    }
    let p = problem();
    let good = "def add_one(n):\n    print('dbg')\n    return n + 1\n";
    let r = d.run_examples(&p, Language::Python, good).await.unwrap();
    assert_eq!(
        (r.kind, r.passed(), r.total()),
        (LocalRunKind::Examples, 2, 2)
    );

    let wrong = "def add_one(n):\n    return n\n";
    let r = d.run_examples(&p, Language::Python, wrong).await.unwrap();
    assert_eq!(r.summary(), "예제 0/2 통과");

    let r = d.run_input(&p, Language::Python, good, "7").await.unwrap();
    assert_eq!(r.summary(), "반환값 8");
    assert!(r.stdout.contains("dbg"), "{:?}", r.stdout);

    assert!(matches!(
        d.run_input(&p, Language::Python, good, "\"x\"").await,
        Err(DockerError::BadInput(_))
    ));

    let slow = "def add_one(n):\n    while True:\n        pass\n";
    let r = d.run_input(&p, Language::Python, slow, "1").await.unwrap();
    assert_eq!(r.summary(), "시간 초과");
}
