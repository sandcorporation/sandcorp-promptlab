//! 소켓 프록시 규칙 시험 — 거부 사례마다 시험 하나, 허용 사례는 실제 docker CLI 본문(픽스처).
//!
//! 판정 함수(`classify`·`check_create`)를 직접 부르는 시험과, 가짜 docker(유닉스 소켓)를 띄워 실제로 넘기는지·attach
//! 양방향 스트림이 되는지 보는 시험이 있다.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use promptlab_docker_proxy::rules::{
    self, Deny, MAX_BODY, Route, allowed_images, check_create, classify,
};
use promptlab_docker_proxy::{Proxy, serve};
use promptlab_judge::{docker_run_args, limits};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const NAME: &str = "promptlab-run-0123456789abcdef0123456789abcdef";
const ID: &str = "7b7f660478583c6f8670003192e6431decba38b8d9e20c4a482052440492989f";

fn query() -> String {
    format!("name={NAME}")
}

/// 실제 CLI 본문, 이미지는 지금 판정기가 쓰는 참조로.
fn base() -> Value {
    let mut v: Value =
        serde_json::from_str(include_str!("fixtures/create-docker-cli-29.8.1.json")).unwrap();
    v["Image"] = Value::String(allowed_images()[0].clone());
    v
}

fn check(v: &Value) -> Result<String, Deny> {
    check_create(
        Some(&query()),
        Some("application/json"),
        &serde_json::to_vec(v).unwrap(),
    )
}

fn rejected(mutate: impl FnOnce(&mut Value)) -> Deny {
    let mut v = base();
    mutate(&mut v);
    check(&v).expect_err("거부여야 한다")
}

fn host(v: &mut Value) -> &mut serde_json::Map<String, Value> {
    v["HostConfig"].as_object_mut().unwrap()
}

// ---------- 허용 ----------

#[test]
fn 판정기_인자의_실제_cli_본문은_모든_러너_이미지에서_받는다() {
    for image in allowed_images() {
        let mut v = base();
        v["Image"] = Value::String(image.clone());
        assert_eq!(check(&v), Ok(NAME.to_owned()), "{image}");
    }
}

#[test]
fn 픽스처는_docker_run_args의_상수와_같다() {
    // 근거 대조: 픽스처를 만든 인자 = docker_run_args, 그 값 = limits 상수.
    let args = docker_run_args("img", NAME).join(" ");
    let v = base();
    let h = &v["HostConfig"];
    assert!(args.contains(&format!("--network {}", limits::NETWORK)));
    assert_eq!(h["NetworkMode"], limits::NETWORK);
    assert!(args.contains(&format!("--memory {}", limits::MEMORY)));
    assert_eq!(h["Memory"], rules::ram_bytes(limits::MEMORY));
    assert_eq!(h["MemorySwap"], rules::ram_bytes(limits::MEMORY));
    assert_eq!(h["NanoCpus"], rules::nano_cpus(limits::CPUS));
    assert_eq!(h["PidsLimit"], limits::PIDS);
    assert_eq!(h["Tmpfs"], rules::expected_tmpfs());
    assert_eq!(v["User"], limits::USER);
    assert_eq!(h["CapDrop"], json!(["ALL"]));
    assert_eq!(h["SecurityOpt"], json!(["no-new-privileges"]));
    assert_eq!(h["ReadonlyRootfs"], true);
    assert_eq!(h["AutoRemove"], true);
    assert!(args.contains("--rm") && args.contains("--read-only"));
}

#[test]
fn 허용_호출과_판_접두() {
    assert_eq!(classify("HEAD", "/_ping"), Ok(Route::Ping));
    assert_eq!(classify("GET", "/v1.56/_ping"), Ok(Route::Ping));
    assert_eq!(classify("GET", "/v1.47/version"), Ok(Route::Version));
    assert_eq!(
        classify("POST", "/v1.56/containers/create"),
        Ok(Route::Create)
    );
    assert_eq!(
        classify("POST", &format!("/v1.56/containers/{ID}/attach")),
        Ok(Route::Attach(ID.into()))
    );
    assert_eq!(
        classify("POST", &format!("/v1.56/containers/{ID}/wait")),
        Ok(Route::Wait(ID.into()))
    );
    assert_eq!(
        classify("POST", &format!("/v1.56/containers/{ID}/start")),
        Ok(Route::Start(ID.into()))
    );
    assert_eq!(
        classify("POST", &format!("/v1.56/containers/{NAME}/kill")),
        Ok(Route::Kill(NAME.into()))
    );
    assert_eq!(
        classify("DELETE", &format!("/containers/{ID}")),
        Ok(Route::Delete(ID.into()))
    );
    assert_eq!(
        classify("GET", &format!("/containers/{NAME}/json")),
        Ok(Route::Inspect(NAME.into()))
    );
}

// ---------- 거부: 경로 ----------

macro_rules! 경로_거부 {
    ($name:ident, $method:expr, $path:expr) => {
        #[test]
        fn $name() {
            let path: String = $path.to_string();
            let r = classify($method, &path);
            assert!(r.is_err(), "{} {path} 가 통과했다: {r:?}", $method);
        }
    };
}

경로_거부!(이미지_pull_거부, "POST", "/v1.56/images/create");
경로_거부!(이미지_build_거부, "POST", "/v1.56/build");
경로_거부!(이미지_조회_거부, "GET", "/v1.56/images/json");
경로_거부!(이미지_삭제_거부, "DELETE", "/v1.56/images/alpine");
경로_거부!(이미지_load_거부, "POST", "/images/load");
경로_거부!(볼륨_만들기_거부, "POST", "/v1.56/volumes/create");
경로_거부!(
    exec_만들기_거부,
    "POST",
    format!("/v1.56/containers/{ID}/exec")
);
경로_거부!(exec_시작_거부, "POST", "/v1.56/exec/abc/start");
경로_거부!(네트워크_만들기_거부, "POST", "/v1.56/networks/create");
경로_거부!(네트워크_연결_거부, "POST", "/v1.56/networks/bridge/connect");
경로_거부!(컨테이너_목록_거부, "GET", "/v1.56/containers/json");
경로_거부!(info_거부, "GET", "/v1.56/info");
경로_거부!(로그_거부, "GET", format!("/v1.56/containers/{ID}/logs"));
경로_거부!(
    자원_바꾸기_거부,
    "POST",
    format!("/v1.56/containers/{ID}/update")
);
경로_거부!(
    파일_넣기_거부,
    "PUT",
    format!("/v1.56/containers/{ID}/archive")
);
경로_거부!(
    파일_꺼내기_거부,
    "GET",
    format!("/v1.56/containers/{ID}/archive")
);
경로_거부!(커밋_거부, "POST", "/v1.56/commit");
경로_거부!(플러그인_거부, "POST", "/v1.56/plugins/pull");
경로_거부!(swarm_거부, "POST", "/v1.56/swarm/init");
경로_거부!(사건_구독_거부, "GET", "/v1.56/events");
경로_거부!(모르는_경로_거부, "GET", "/v1.56/whatever");
경로_거부!(메서드가_다르면_거부, "DELETE", "/v1.56/containers/create");
경로_거부!(ping_post_거부, "POST", "/_ping");
경로_거부!(
    판정기_밖_컨테이너_이름_거부,
    "POST",
    "/v1.56/containers/promptlab-prod-db-1/kill"
);
경로_거부!(짧은_id_거부, "POST", "/v1.56/containers/7b7f66047858/kill");
경로_거부!(
    대문자_hex_이름_거부,
    "POST",
    "/containers/promptlab-run-0123456789ABCDEF0123456789ABCDEF/kill"
);
경로_거부!(판_접두_우회_이미지_거부, "POST", "/v1.47/images/create");
경로_거부!(낮은_판_접두_거부, "POST", "/v1.23/containers/create");
경로_거부!(판_접두_꼴_틀림_거부, "POST", "/v1.47.0/containers/create");
경로_거부!(판_2_접두_거부, "POST", "/v2/containers/create");
경로_거부!(판_접두만_거부, "GET", "/v1.56");
경로_거부!(점점_우회_거부, "POST", "/containers/../images/create");
경로_거부!(판_뒤_점점_우회_거부, "POST", "/v1.56/../images/create");
경로_거부!(점_조각_거부, "POST", "/./containers/create");
경로_거부!(인코딩_우회_거부, "POST", "/containers/%2e%2e/images/create");
경로_거부!(인코딩_이름_거부, "POST", "/containers/cre%61te");
경로_거부!(빈_조각_거부, "POST", "//containers/create");
경로_거부!(끝_빗금_거부, "POST", "/containers/create/");
경로_거부!(역빗금_거부, "POST", "/containers\\create");

// ---------- 거부: 만들기 질의·머리·꼴 ----------

#[test]
fn 이름_없는_만들기_거부() {
    let b = serde_json::to_vec(&base()).unwrap();
    assert!(check_create(None, Some("application/json"), &b).is_err());
}

#[test]
fn 판정기_꼴_아닌_이름_거부() {
    let b = serde_json::to_vec(&base()).unwrap();
    assert!(check_create(Some("name=evil"), Some("application/json"), &b).is_err());
}

#[test]
fn 질의_덧붙임_거부() {
    let b = serde_json::to_vec(&base()).unwrap();
    let q = format!("name={NAME}&platform=linux/amd64");
    assert!(check_create(Some(&q), Some("application/json"), &b).is_err());
}

#[test]
fn json_아닌_content_type_거부() {
    let b = serde_json::to_vec(&base()).unwrap();
    assert!(
        check_create(
            Some(&query()),
            Some("application/x-www-form-urlencoded"),
            &b
        )
        .is_err()
    );
    assert!(check_create(Some(&query()), None, &b).is_err());
}

#[test]
fn 큰_본문_거부() {
    let mut v = base();
    v["Hostname"] = Value::String("x".repeat(MAX_BODY));
    assert!(check(&v).is_err());
}

#[test]
fn 뒤에_붙은_글자_거부() {
    let mut b = serde_json::to_vec(&base()).unwrap();
    b.extend_from_slice(b" {}");
    assert!(check_create(Some(&query()), Some("application/json"), &b).is_err());
}

#[test]
fn 객체_아닌_본문_거부() {
    assert!(check_create(Some(&query()), Some("application/json"), b"[]").is_err());
}

#[test]
fn 같은_키_겹침_거부() {
    // Go는 뒤의 것을 쓴다 — 앞의 Privileged=false만 보고 통과시키면 안 된다.
    let s = serde_json::to_string(&base()).unwrap().replacen(
        "\"Privileged\":false",
        "\"Privileged\":false,\"Privileged\":true",
        1,
    );
    assert!(check_create(Some(&query()), Some("application/json"), s.as_bytes()).is_err());
}

#[test]
fn 대소문자만_다른_겹친_키_거부() {
    // Go는 필드 이름을 대소문자 무시로 맞춘다 — 뒤의 readonlyrootfs=false가 ReadonlyRootfs를 덮는다.
    let s = serde_json::to_string(&base()).unwrap().replacen(
        "\"ReadonlyRootfs\":true",
        "\"ReadonlyRootfs\":true,\"readonlyrootfs\":false",
        1,
    );
    assert!(check_create(Some(&query()), Some("application/json"), s.as_bytes()).is_err());
}

#[test]
fn 대소문자_다른_알려진_키_거부() {
    let d = rejected(|v| {
        host(v).insert("privileged".into(), json!(false));
    });
    assert!(d.0.contains("대소문자"), "{d}");
}

#[test]
fn 유니코드_접기_키_거부() {
    // Go의 접기는 ſ(U+017F)를 s로 본다 — "ſecurityOpt": null 이 SecurityOpt를 지운다.
    let d = rejected(|v| {
        host(v).insert("ſecurityOpt".into(), Value::Null);
    });
    assert!(d.0.contains("ASCII"), "{d}");
}

#[test]
fn 이스케이프_키도_같은_키로_본다() {
    let s = serde_json::to_string(&base()).unwrap().replacen(
        "\"Privileged\":false",
        "\"Privileged\":false,\"\\u0050rivileged\":true",
        1,
    );
    assert!(check_create(Some(&query()), Some("application/json"), s.as_bytes()).is_err());
}

// ---------- 거부: 만들기 본문 ----------

macro_rules! 본문_거부 {
    ($name:ident, |$v:ident| $body:expr) => {
        #[test]
        fn $name() {
            let d = rejected(|$v: &mut Value| {
                $body;
            });
            assert!(!d.0.is_empty());
        }
    };
}

본문_거부!(privileged_거부, |v| host(v)
    .insert("Privileged".into(), json!(true)));
본문_거부!(cap_add_거부, |v| host(v)
    .insert("CapAdd".into(), json!(["SYS_ADMIN"])));
본문_거부!(cap_drop_없음_거부, |v| host(v).remove("CapDrop"));
본문_거부!(cap_drop_일부_거부, |v| host(v)
    .insert("CapDrop".into(), json!(["NET_RAW"])));
본문_거부!(binds_거부, |v| host(v)
    .insert("Binds".into(), json!(["/:/host"])));
본문_거부!(bind_타입_mounts_거부, |v| host(v).insert(
    "Mounts".into(),
    json!([{"Type": "bind", "Source": "/var/run/docker.sock", "Target": "/s"}])
));
본문_거부!(volume_타입_mounts_거부, |v| host(v).insert(
    "Mounts".into(),
    json!([{"Type": "volume", "Source": "x", "Target": "/x"}])
));
본문_거부!(tmpfs_타입_mounts도_거부, |v| host(v).insert(
    "Mounts".into(),
    json!([{"Type": "tmpfs", "Target": "/x"}])
));
본문_거부!(volumes_from_거부, |v| host(v)
    .insert("VolumesFrom".into(), json!(["promptlab-prod-server-1"])));
본문_거부!(config_volumes_거부, |v| v["Volumes"] =
    json!({"/data": {}}));
본문_거부!(network_bridge_거부, |v| host(v)
    .insert("NetworkMode".into(), json!("bridge")));
본문_거부!(network_host_거부, |v| host(v)
    .insert("NetworkMode".into(), json!("host")));
본문_거부!(network_다른_컨테이너_거부, |v| host(v).insert(
    "NetworkMode".into(),
    json!("container:promptlab-prod-db-1")
));
본문_거부!(network_없음_거부, |v| host(v).remove("NetworkMode"));
본문_거부!(networking_config_거부, |v| v["NetworkingConfig"] =
    json!({"EndpointsConfig": {"bridge": {}}}));
본문_거부!(허용_밖_이미지_거부, |v| v["Image"] =
    json!("alpine:latest"));
본문_거부!(비슷한_이름_이미지_거부, |v| v["Image"] =
    json!("ghcr.io/sandcorporation/promptlab-runner-python:latest"));
본문_거부!(이미지_없음_거부, |v| v
    .as_object_mut()
    .unwrap()
    .remove("Image"));
본문_거부!(user_root_거부, |v| v["User"] = json!("0"));
본문_거부!(user_빈값_거부, |v| v["User"] = json!(""));
본문_거부!(user_uid만_거부, |v| v["User"] = json!("65534"));
본문_거부!(readonly_거짓_거부, |v| host(v)
    .insert("ReadonlyRootfs".into(), json!(false)));
본문_거부!(readonly_없음_거부, |v| host(v).remove("ReadonlyRootfs"));
본문_거부!(tmpfs_다른_경로_거부, |v| host(v).insert(
    "Tmpfs".into(),
    json!({"/work": "size=64m,exec,mode=1777", "/etc": "size=16m"})
));
본문_거부!(tmpfs_경로_추가_거부, |v| host(v)["Tmpfs"]["/var"] =
    json!("size=1m"));
본문_거부!(tmpfs_옵션_다름_거부, |v| host(v)["Tmpfs"]["/work"] =
    json!("exec,mode=1777"));
본문_거부!(memory_다름_거부, |v| host(v)
    .insert("Memory".into(), json!(1_073_741_824i64)));
본문_거부!(memory_없음_거부, |v| host(v).remove("Memory"));
본문_거부!(memory_swap_무제한_거부, |v| host(v)
    .insert("MemorySwap".into(), json!(-1)));
본문_거부!(nano_cpus_다름_거부, |v| host(v)
    .insert("NanoCpus".into(), json!(2_000_000_000i64)));
본문_거부!(nano_cpus_없음_거부, |v| host(v).remove("NanoCpus"));
본문_거부!(cpu_quota_거부, |v| host(v)
    .insert("CpuQuota".into(), json!(200000)));
본문_거부!(pids_무제한_거부, |v| host(v)
    .insert("PidsLimit".into(), json!(-1)));
본문_거부!(pids_없음_거부, |v| host(v).remove("PidsLimit"));
본문_거부!(ulimits_다름_거부, |v| host(v).insert(
    "Ulimits".into(),
    json!([{"Name": "nofile", "Soft": 65536, "Hard": 65536}])
));
본문_거부!(ulimits_더함_거부, |v| host(v)["Ulimits"]
    .as_array_mut()
    .unwrap()
    .push(json!({"Name": "nproc", "Soft": 1, "Hard": 1})));
본문_거부!(security_opt_seccomp_해제_거부, |v| host(v).insert(
    "SecurityOpt".into(),
    json!(["no-new-privileges", "seccomp=unconfined"])
));
본문_거부!(security_opt_apparmor_해제_거부, |v| host(v)
    .insert("SecurityOpt".into(), json!(["apparmor=unconfined"])));
본문_거부!(pid_mode_host_거부, |v| host(v)
    .insert("PidMode".into(), json!("host")));
본문_거부!(ipc_mode_host_거부, |v| host(v)
    .insert("IpcMode".into(), json!("host")));
본문_거부!(uts_mode_host_거부, |v| host(v)
    .insert("UTSMode".into(), json!("host")));
본문_거부!(userns_mode_host_거부, |v| host(v)
    .insert("UsernsMode".into(), json!("host")));
본문_거부!(cgroupns_host_거부, |v| host(v)
    .insert("CgroupnsMode".into(), json!("host")));
본문_거부!(cgroup_parent_거부, |v| host(v)
    .insert("CgroupParent".into(), json!("/")));
본문_거부!(devices_거부, |v| host(v).insert(
    "Devices".into(),
    json!([{"PathOnHost": "/dev/sda", "PathInContainer": "/dev/sda", "CgroupPermissions": "rwm"}])
));
본문_거부!(device_cgroup_rules_거부, |v| host(v)
    .insert("DeviceCgroupRules".into(), json!(["a *:* rwm"])));
본문_거부!(device_requests_거부, |v| host(v).insert(
    "DeviceRequests".into(),
    json!([{"Driver": "nvidia", "Count": -1}])
));
본문_거부!(links_거부, |v| host(v)
    .insert("Links".into(), json!(["promptlab-prod-db-1:db"])));
본문_거부!(sysctls_거부, |v| host(v)
    .insert("Sysctls".into(), json!({"net.ipv4.ip_forward": "1"})));
본문_거부!(runtime_거부, |v| host(v)
    .insert("Runtime".into(), json!("runc-unsafe")));
본문_거부!(extra_hosts_거부, |v| host(v)
    .insert("ExtraHosts".into(), json!(["db:10.0.0.1"])));
본문_거부!(port_bindings_거부, |v| host(v).insert(
    "PortBindings".into(),
    json!({"80/tcp": [{"HostPort": "80"}]})
));
본문_거부!(publish_all_거부, |v| host(v)
    .insert("PublishAllPorts".into(), json!(true)));
본문_거부!(oom_kill_disable_거부, |v| host(v)
    .insert("OomKillDisable".into(), json!(true)));
본문_거부!(oom_score_거부, |v| host(v)
    .insert("OomScoreAdj".into(), json!(-1000)));
본문_거부!(init_거부, |v| host(v).insert("Init".into(), json!(true)));
본문_거부!(masked_paths_해제_거부, |v| host(v)
    .insert("MaskedPaths".into(), json!([])));
본문_거부!(readonly_paths_해제_거부, |v| host(v)
    .insert("ReadonlyPaths".into(), json!([])));
본문_거부!(group_add_거부, |v| host(v)
    .insert("GroupAdd".into(), json!(["999"])));
본문_거부!(log_driver_거부, |v| host(v).insert(
    "LogConfig".into(),
    json!({"Type": "syslog", "Config": {}})
));
본문_거부!(restart_policy_거부, |v| host(v)
    .insert("RestartPolicy".into(), json!({"Name": "always"})));
본문_거부!(auto_remove_꺼짐_거부, |v| host(v)
    .insert("AutoRemove".into(), json!(false)));
본문_거부!(shm_size_거부, |v| host(v)
    .insert("ShmSize".into(), json!(1_073_741_824i64)));
본문_거부!(storage_opt_거부, |v| host(v)
    .insert("StorageOpt".into(), json!({"size": "100G"})));
본문_거부!(모르는_host_키_거부, |v| host(v)
    .insert("SomethingNew".into(), json!("x")));
본문_거부!(모르는_host_키_배열_거부, |v| host(v)
    .insert("SomethingNew".into(), json!([])));
본문_거부!(host_config_없음_거부, |v| v
    .as_object_mut()
    .unwrap()
    .remove("HostConfig"));
본문_거부!(entrypoint_거부, |v| v["Entrypoint"] = json!(["/bin/sh"]));
본문_거부!(cmd_거부, |v| v["Cmd"] = json!(["sh", "-c", "id"]));
본문_거부!(tty_거부, |v| v["Tty"] = json!(true));
본문_거부!(env_거부, |v| v["Env"] = json!(["LD_PRELOAD=/tmp/x.so"]));
본문_거부!(working_dir_거부, |v| v["WorkingDir"] = json!("/"));
본문_거부!(healthcheck_거부, |v| v["Healthcheck"] =
    json!({"Test": ["CMD", "sh"]}));
본문_거부!(labels_거부, |v| v["Labels"] =
    json!({"com.docker.compose.project": "promptlab-prod"}));
본문_거부!(모르는_맨위_키_거부, |v| v["Surprise"] =
    json!({"a": 1}));

// ---------- 가짜 docker로 실제 중계 ----------

type Seen = Arc<Mutex<Vec<String>>>;

async fn fake_docker(socket: std::path::PathBuf, seen: Seen) {
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let seen = seen.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |mut req: Request<Incoming>| {
                let seen = seen.clone();
                async move {
                    let path = req.uri().path().to_owned();
                    seen.lock()
                        .unwrap()
                        .push(format!("{} {}", req.method(), path));
                    let resp = if path.ends_with("/_ping") {
                        Response::new(Full::new(Bytes::from_static(b"OK")))
                    } else if path.ends_with("/containers/create") {
                        let mut r = Response::new(Full::new(Bytes::from(
                            json!({"Id": ID, "Warnings": []}).to_string(),
                        )));
                        *r.status_mut() = StatusCode::CREATED;
                        r
                    } else if path.ends_with("/attach") {
                        let up = hyper::upgrade::on(&mut req);
                        tokio::spawn(async move {
                            // 받은 것을 끝(쓰기 닫힘)까지 읽고 그대로 돌려준 뒤 닫는다.
                            let mut io = TokioIo::new(up.await.unwrap());
                            let mut buf = Vec::new();
                            io.read_to_end(&mut buf).await.unwrap();
                            io.write_all(&buf).await.unwrap();
                            io.shutdown().await.unwrap();
                        });
                        let mut r = Response::new(Full::new(Bytes::new()));
                        *r.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                        r.headers_mut()
                            .insert("connection", "Upgrade".parse().unwrap());
                        r.headers_mut().insert("upgrade", "tcp".parse().unwrap());
                        r
                    } else {
                        let mut r = Response::new(Full::new(Bytes::new()));
                        *r.status_mut() = StatusCode::NO_CONTENT;
                        r
                    };
                    Ok::<_, std::convert::Infallible>(resp)
                }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), svc)
                .with_upgrades()
                .await;
        });
    }
}

async fn client(
    addr: std::net::SocketAddr,
) -> hyper::client::conn::http1::SendRequest<Full<Bytes>> {
    let s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(s))
        .await
        .unwrap();
    tokio::spawn(conn.with_upgrades());
    sender
}

async fn call(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<Vec<u8>>,
) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "x");
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    let req = b
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .unwrap();
    let resp = client(addr).await.send_request(req).await.unwrap();
    let status = resp.status();
    let text = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&text).into_owned())
}

#[tokio::test]
async fn 가짜_docker로_허용만_넘기고_attach_스트림은_양방향() {
    let dir = std::env::temp_dir().join(format!("pl-proxy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("docker.sock");
    let _ = std::fs::remove_file(&socket);
    let seen: Seen = Arc::default();
    tokio::spawn(fake_docker(socket.clone(), seen.clone()));
    while !socket.exists() {
        tokio::task::yield_now().await;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Arc::new(Proxy::new(&socket))));

    // 허용: ping
    let (s, body) = call(addr, "GET", "/v1.56/_ping", None).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "OK"));

    // 거부: pull — 소켓까지 가지 않는다.
    let (s, body) = call(addr, "POST", "/v1.56/images/create?fromImage=alpine", None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(body.contains("promptlab 소켓 프록시"), "{body}");

    // 거부: 만들지 않은 컨테이너 시작.
    let (s, _) = call(addr, "POST", &format!("/v1.56/containers/{ID}/start"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // 거부: 특권 만들기.
    let mut bad = base();
    host(&mut bad).insert("Privileged".into(), json!(true));
    let (s, _) = call(
        addr,
        "POST",
        &format!("/v1.56/containers/create?name={NAME}"),
        Some(serde_json::to_vec(&bad).unwrap()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // 허용: 만들기 → 그 id로 시작.
    let (s, body) = call(
        addr,
        "POST",
        &format!("/v1.56/containers/create?name={NAME}"),
        Some(serde_json::to_vec(&base()).unwrap()),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    assert!(body.contains(ID));
    let (s, _) = call(addr, "POST", &format!("/v1.56/containers/{ID}/start"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(
        addr,
        "POST",
        &format!("/v1.56/containers/{NAME}/kill"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // 허용: attach — 업그레이드 뒤 stdin을 보내고 쓰기를 닫으면 같은 바이트가 돌아온다.
    let req = Request::builder()
        .method("POST")
        .uri(format!(
            "/v1.56/containers/{ID}/attach?stderr=1&stdin=1&stdout=1&stream=1"
        ))
        .header("host", "x")
        .header("connection", "Upgrade")
        .header("upgrade", "tcp")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let resp = client(addr).await.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut io = TokioIo::new(hyper::upgrade::on(resp).await.unwrap());
    let payload = vec![b'x'; 200_000];
    io.write_all(&payload).await.unwrap();
    io.shutdown().await.unwrap();
    let mut back = Vec::new();
    io.read_to_end(&mut back).await.unwrap();
    assert_eq!(back.len(), payload.len());

    // 거부한 것은 소켓에 닿지 않았다.
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter()
            .all(|s| !s.contains("/images/") && !s.contains("Privileged")),
        "{seen:?}"
    );
    assert_eq!(
        seen.iter()
            .filter(|s| s.ends_with("/containers/create"))
            .count(),
        1,
        "{seen:?}"
    );
}
