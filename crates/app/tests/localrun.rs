//! 로컬 실행(#97)의 docker 없는 부분: docker 오류 → 사람 말, 이미지 받기 진행, 「실행」 입력 읽기, 결과 요약·로그 사건.

use promptlab_app::localrun::{
    DockerError, INSTALL_URL, LICENSE_NOTE, PullProgress, explain, failed_event, input_example,
    parse_input, report,
};
use promptlab_judge::{CaseResult, CompileInfo, RunOutcome, Status};
use promptlab_shared::coding::{Language, Param, Signature, ValueType};
use promptlab_shared::log::{LocalRunKind, LogEvent};
use serde_json::{Value, json};

#[test]
fn docker_오류를_사람_말로_옮긴다() {
    let cases: &[(&str, DockerError)] = &[
        (
            "docker를 띄우지 못했다: No such file or directory (os error 2)",
            DockerError::NotInstalled,
        ),
        (
            "'docker' is not recognized as an internal or external command",
            DockerError::NotInstalled,
        ),
        (
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
            DockerError::DaemonDown,
        ),
        (
            "error during connect: Get \"http://%2F%2F.%2Fpipe%2FdockerDesktopLinuxEngine/v1.47/version\": open //./pipe/dockerDesktopLinuxEngine: The system cannot find the file specified.",
            DockerError::DaemonDown,
        ),
        (
            "permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock",
            DockerError::PermissionDenied,
        ),
        (
            "no matching manifest for windows/amd64 10.0.22631 in the manifest list entries",
            DockerError::WindowsContainers,
        ),
        (
            "Unable to find image 'ghcr.io/sandcorporation/promptlab-runner-java@sha256:abc' locally",
            DockerError::ImageMissing(Language::Java),
        ),
        (
            "Error: No such image: ghcr.io/sandcorporation/promptlab-runner-cpp@sha256:abc",
            DockerError::ImageMissing(Language::Cpp),
        ),
    ];
    for (raw, want) in cases {
        assert_eq!(&explain(raw), want, "{raw}");
    }
    assert!(matches!(explain("뭔가 이상하다"), DockerError::Failed(m) if m == "뭔가 이상하다"));
    // 사람 말: 무엇이 문제이고 무엇을 하면 되는지.
    assert!(
        DockerError::DaemonDown
            .to_string()
            .contains("Docker Desktop을 켜고")
    );
    assert!(
        DockerError::WindowsContainers
            .to_string()
            .contains("Switch to Linux containers")
    );
    assert!(DockerError::TimedOut.to_string().contains("다시 시도"));
    assert!(
        DockerError::ImageMissing(Language::Java)
            .to_string()
            .starts_with(&format!("{} 실행 이미지", Language::Java.display_name()))
    );
}

#[test]
fn 설치_안내와_라이선스_문장() {
    assert!(INSTALL_URL.starts_with("https://docs.docker.com/"));
    assert_eq!(
        LICENSE_NOTE,
        "Docker Desktop은 개인 사용은 무료이고, 일정 규모 이상 기업의 업무용 사용은 유료 요금제가 필요합니다."
    );
}

#[test]
fn 이미지_받기_진행은_층을_센다() {
    let mut p = PullProgress::new();
    assert_eq!(p.text(), "준비 중");
    for line in [
        "sha256-pinned: Pulling from sandcorporation/promptlab-runner-python",
        "a1b2c3d4e5f6: Pulling fs layer",
        "0f1e2d3c4b5a: Pulling fs layer",
        "99aa88bb77cc: Already exists",
        "a1b2c3d4e5f6: Downloading [=====>   ]  1.2MB/3MB",
        "a1b2c3d4e5f6: Pull complete",
        "a1b2c3d4e5f6: Pull complete",
        "Digest: sha256:abc",
    ] {
        p.line(line);
    }
    assert_eq!((p.done(), p.layers()), (2, 3));
    assert_eq!(p.text(), "층 2/3");
}

fn sig(params: &[ValueType]) -> Signature {
    Signature {
        name: "f".into(),
        params: params
            .iter()
            .enumerate()
            .map(|(i, &ty)| Param {
                name: format!("a{i}"),
                ty,
            })
            .collect(),
        returns: ValueType::Int,
    }
}

#[test]
fn 실행_입력은_json_배열이고_인자가_하나면_값만_써도_된다() {
    let two = sig(&[ValueType::IntArray, ValueType::Int]);
    assert_eq!(input_example(&two), "[[],0]");
    assert_eq!(
        parse_input(&two, " [[2,7,11,15], 9] ").unwrap(),
        vec![json!([2, 7, 11, 15]), json!(9)]
    );
    // 비우면 기본값.
    assert_eq!(parse_input(&two, "").unwrap(), vec![json!([]), json!(0)]);
    assert!(
        matches!(parse_input(&two, "[1]"), Err(DockerError::BadInput(m)) if m.contains("인자는 2개"))
    );
    assert!(
        matches!(parse_input(&two, "[1, 2]"), Err(DockerError::BadInput(m)) if m.contains("a0"))
    );
    assert!(matches!(
        parse_input(&two, "{"),
        Err(DockerError::BadInput(_))
    ));

    let one = sig(&[ValueType::IntArray]);
    assert_eq!(
        parse_input(&one, "[1,2,3]").unwrap(),
        vec![json!([1, 2, 3])]
    );
    assert_eq!(
        parse_input(&one, "[[1,2,3]]").unwrap(),
        vec![json!([1, 2, 3])]
    );
}

fn case(status: Status, value: Option<Value>) -> CaseResult {
    CaseResult {
        status,
        value,
        message: None,
        ms: 3,
    }
}

#[test]
fn 예제_테스트_결과는_통과_수와_사건이_되고_코드는_싣지_않는다() {
    let cases = vec![vec![json!(1)], vec![json!(2)]];
    let expected = vec![json!(2), json!(3)];
    let outcome = RunOutcome {
        compile: None,
        cases: vec![
            case(Status::Ok, Some(json!(2))),
            case(Status::Ok, Some(json!(4))),
        ],
        stdout: "dbg\n".into(),
        stderr: String::new(),
        wall_killed: false,
    };
    let r = report(
        LocalRunKind::Examples,
        Language::Python,
        &cases,
        Some((ValueType::Int, &expected)),
        &outcome,
    );
    assert_eq!((r.passed(), r.total()), (1, 2));
    assert_eq!(r.summary(), "예제 1/2 통과");
    assert_eq!(r.cases[1].ok, Some(false));
    assert_eq!(r.cases[1].expected.as_deref(), Some("3"));
    assert_eq!(r.cases[1].got.as_deref(), Some("4"));
    assert_eq!(r.stdout, "dbg\n");
    assert_eq!(
        r.event(),
        LogEvent::LocalRun {
            kind: LocalRunKind::Examples,
            passed: Some(1),
            total: Some(2),
            summary: "예제 1/2 통과".into(),
        }
    );
}

#[test]
fn 실행_결과는_반환값_또는_상태를_요약한다() {
    let cases = vec![vec![json!(1)]];
    let ok = RunOutcome {
        compile: None,
        cases: vec![case(Status::Ok, Some(json!([0, 1])))],
        stdout: String::new(),
        stderr: String::new(),
        wall_killed: false,
    };
    let r = report(LocalRunKind::Run, Language::Java, &cases, None, &ok);
    assert_eq!(r.summary(), "반환값 [0,1]");
    assert!(matches!(
        r.event(),
        LogEvent::LocalRun {
            passed: None,
            total: None,
            ..
        }
    ));

    let slow = RunOutcome {
        cases: vec![case(Status::TimeLimit, None)],
        ..ok.clone()
    };
    assert_eq!(
        report(LocalRunKind::Run, Language::Java, &cases, None, &slow).summary(),
        "시간 초과"
    );

    let broken = RunOutcome {
        compile: Some(CompileInfo {
            ok: false,
            message: "Main.java:3: error: ';' expected".into(),
        }),
        cases: vec![case(Status::CompileError, None)],
        ..ok
    };
    let r = report(LocalRunKind::Run, Language::Java, &cases, None, &broken);
    assert_eq!(r.summary(), "컴파일 오류");
    assert_eq!(
        r.compile_error.as_deref(),
        Some("Main.java:3: error: ';' expected")
    );
}

#[test]
fn docker를_못_써도_눌렀다는_사건은_남는다() {
    assert_eq!(
        failed_event(LocalRunKind::Examples, &DockerError::DaemonDown),
        LogEvent::LocalRun {
            kind: LocalRunKind::Examples,
            passed: None,
            total: None,
            summary: "Docker 꺼짐".into(),
        }
    );
}
