//! 끝에서 끝 — 호스트 docker가 있어야 한다(`#[ignore]`, CI 이미지에는 docker가 없다).
//!
//! ```text
//! bash scripts/build-runners.sh --local
//! cargo test -p promptlab-docker-proxy --test e2e -- --ignored
//! ```
//!
//! 프록시를 이 프로세스 안에서 호스트 소켓(`/var/run/docker.sock`, `PROMPTLAB_DOCKER_SOCKET`로 바꾼다)에 붙여 띄우고,
//! `DOCKER_HOST=tcp://127.0.0.1:<포트>`로 판정기 lib(Python 한 문제)이 돌고, 특권·바인드·pull·목록은 프록시가 거부하는지
//! docker CLI로 본다.

use std::net::SocketAddr;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use promptlab_docker_proxy::{Proxy, serve};
use promptlab_judge::{Judge, RunRequest, Status, docker_run_args};
use promptlab_shared::coding::{self, Language, Param, Signature, ValueType};
use serde_json::json;

fn proxy() -> SocketAddr {
    static ADDR: OnceLock<SocketAddr> = OnceLock::new();
    *ADDR.get_or_init(|| {
        let socket = std::env::var("PROMPTLAB_DOCKER_SOCKET")
            .unwrap_or_else(|_| "/var/run/docker.sock".into());
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = std_listener.local_addr().unwrap();
        std_listener.set_nonblocking(true).unwrap();
        // 시험마다 런타임이 따로라 프록시는 자기 스레드·런타임에 둔다.
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
                serve(listener, Arc::new(Proxy::new(socket))).await.unwrap();
            });
        });
        // SAFETY: 이 시험 바이너리에서 환경변수를 바꾸는 곳은 여기 하나(OnceLock 안)이고, 판정기가 띄우는 docker CLI가
        // 이 값을 물려받는다.
        unsafe { std::env::set_var("DOCKER_HOST", format!("tcp://{addr}")) };
        addr
    })
}

/// 프록시를 거쳐 docker CLI를 부른다 → (성공, stderr).
fn docker(args: &[&str]) -> (bool, String) {
    let addr = proxy();
    let out = Command::new("docker")
        .args(args)
        .env("DOCKER_HOST", format!("tcp://{addr}"))
        .output()
        .expect("docker CLI");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn python_image() -> String {
    coding::runner_image(Language::Python).reference()
}

fn assert_refused(args: &[&str]) {
    let (ok, err) = docker(args);
    assert!(!ok, "{args:?} 가 통과했다");
    assert!(
        err.contains("promptlab 소켓 프록시"),
        "{args:?}: 프록시 거부가 아니다: {err}"
    );
}

#[tokio::test]
#[ignore = "호스트 docker·로컬 러너 이미지 필요"]
async fn 프록시를_거쳐_판정기가_python_한_문제를_돈다() {
    proxy();
    let sig = Signature {
        name: "add".into(),
        params: vec![
            Param {
                name: "a".into(),
                ty: ValueType::Int,
            },
            Param {
                name: "b".into(),
                ty: ValueType::Int,
            },
        ],
        returns: ValueType::Int,
    };
    let t = coding::solution_template(Language::Python, &sig);
    let start = t.find("answer = ").unwrap() + "answer = ".len();
    let end = start + t[start..].find('\n').unwrap();
    let code = format!("{}a + b{}", &t[..start], &t[end..]);
    let out = Judge::new("docker", 1)
        .run(&RunRequest {
            language: Language::Python,
            code,
            signature: sig,
            cases: vec![vec![json!(1), json!(2)], vec![json!(-5), json!(5)]],
            case_time_ms: None,
        })
        .await
        .expect("판정기 실행");
    assert!(!out.wall_killed);
    assert_eq!(
        out.cases.iter().map(|c| c.status).collect::<Vec<_>>(),
        vec![Status::Ok, Status::Ok]
    );
    assert_eq!(out.cases[0].value, Some(json!(3)));
    assert_eq!(out.cases[1].value, Some(json!(0)));
}

#[test]
#[ignore = "호스트 docker 필요"]
fn 프록시는_특권_실행을_거부한다() {
    let image = python_image();
    let mut args: Vec<String> =
        docker_run_args(&image, "promptlab-run-00000000000000000000000000000001");
    let at = args.len() - 1;
    args.insert(at, "--privileged".into());
    assert_refused(&args.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
#[ignore = "호스트 docker 필요"]
fn 프록시는_바인드_마운트를_거부한다() {
    let image = python_image();
    let mut args: Vec<String> =
        docker_run_args(&image, "promptlab-run-00000000000000000000000000000002");
    let at = args.len() - 1;
    args.splice(at..at, ["-v".to_string(), "/:/host".to_string()]);
    assert_refused(&args.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
#[ignore = "호스트 docker 필요"]
fn 프록시는_허용_밖_이미지를_거부한다() {
    let args = docker_run_args(
        "alpine:latest",
        "promptlab-run-00000000000000000000000000000003",
    );
    assert_refused(&args.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
#[ignore = "호스트 docker 필요"]
fn 프록시는_pull을_거부한다() {
    assert_refused(&["pull", "alpine:latest"]);
}

#[test]
#[ignore = "호스트 docker 필요"]
fn 프록시는_목록_조회를_거부한다() {
    assert_refused(&["ps"]);
}

#[test]
#[ignore = "호스트 docker 필요"]
fn 프록시는_남의_컨테이너_kill을_거부한다() {
    assert_refused(&["kill", "promptlab-run-00000000000000000000000000000004"]);
}
