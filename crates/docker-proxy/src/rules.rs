//! 허용 목록 규칙 — 경로 판정([`classify`])과 컨테이너 만들기 본문 검사([`check_create`]).
//!
//! 근거는 판정기가 실제로 부르는 것 하나다: `docker` CLI가 [`promptlab_judge::docker_run_args`]로 `run`할 때와
//! `docker kill <이름>`(벽시계)일 때 보내는 호출(실측, docker CLI 29.8.1 — `tests/fixtures/README.md`):
//!
//! | 호출 | 왜 |
//! |---|---|
//! | `HEAD`·`GET /_ping` | CLI가 API 판을 맞춘다 |
//! | `GET /version` | `docker version`(진단). 읽기 전용, 비밀 없음 |
//! | `POST /containers/create?name=…` | `run` — 본문을 [`check_create`]가 검사 |
//! | `POST /containers/{id}/attach` | `run -i` — stdin·stdout·stderr(HTTP 업그레이드 뒤 양방향 스트림) |
//! | `POST /containers/{id}/wait` | `run --rm`이 끝을 기다린다 |
//! | `POST /containers/{id}/start` | `run` |
//! | `POST /containers/{id}/kill` | 벽시계 `docker kill` |
//! | `DELETE /containers/{id}` | `--rm`(옛 데몬) — 지금 CLI는 `AutoRemove`를 쓰지만 막을 까닭이 없다 |
//! | `GET /containers/{id}/json` | 조회(CLI가 run 도중 부를 수 있다) |
//!
//! 그 밖(이미지 pull·build·inspect, 볼륨·네트워크, exec, 로그, `info` …)은 전부 거부한다. `{id}`는 판정기가 붙이는
//! 이름(`promptlab-run-<32 hex>`)이나 64자 id 꼴만 받고, 프록시는 다시 「이 프록시로 만든 컨테이너」인지 본다
//! ([`crate::Owned`]) — 서버·DB 컨테이너를 조회(환경변수의 비밀)·kill·삭제하지 못하게.
//!
//! 본문은 docker(Go `encoding/json`)와 다르게 읽히면 안 된다: Go는 필드 이름을 대소문자 무시로 맞추고(`ſ`·`K` 같은
//! 유니코드 접기까지) 같은 키가 겹치면 뒤의 것을 쓴다. 그래서 [`parse_strict`]가 ASCII가 아닌 키와 대소문자만 다른 겹친
//! 키를 거부하고, 알려진 키의 대소문자가 다르면 거부한다.

use std::collections::HashSet;
use std::fmt;

use promptlab_judge::limits;
use promptlab_shared::coding::RUNNER_IMAGES;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value, json};

/// 컨테이너 만들기 본문 상한(실제 CLI 본문은 약 1.6KB).
pub const MAX_BODY: usize = 64 * 1024;
/// 받는 API 판 접두의 최소(`/v1.40/` 이상). 옛 판은 만들기 본문을 다르게 읽는 호환 경로가 있다.
pub const MIN_API_MINOR: u32 = 40;
/// 판정기 컨테이너 이름 접두(`promptlab_judge`가 `promptlab-run-<uuid simple>`로 붙인다).
pub const CONTAINER_PREFIX: &str = "promptlab-run-";

/// 거부 까닭(응답 본문·로그 한 줄). 본문 원문은 싣지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deny(pub String);

impl fmt::Display for Deny {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn deny<T>(reason: impl Into<String>) -> Result<T, Deny> {
    Err(Deny(reason.into()))
}

/// 허용한 호출.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Ping,
    Version,
    Create,
    Attach(String),
    Start(String),
    Wait(String),
    Kill(String),
    Delete(String),
    Inspect(String),
}

impl Route {
    /// 대상 컨테이너(있으면).
    pub fn container(&self) -> Option<&str> {
        match self {
            Route::Attach(id)
            | Route::Start(id)
            | Route::Wait(id)
            | Route::Kill(id)
            | Route::Delete(id)
            | Route::Inspect(id) => Some(id),
            Route::Ping | Route::Version | Route::Create => None,
        }
    }

    /// HTTP 업그레이드(hijack)를 받는 호출.
    pub fn is_attach(&self) -> bool {
        matches!(self, Route::Attach(_))
    }
}

/// 판정기 컨테이너 이름(`promptlab-run-` + 소문자 hex 32자).
pub fn is_container_name(s: &str) -> bool {
    s.strip_prefix(CONTAINER_PREFIX)
        .is_some_and(|h| h.len() == 32 && is_lower_hex(h))
}

/// 컨테이너 참조 꼴: 판정기 이름 또는 64자 id.
pub fn is_container_ref(s: &str) -> bool {
    is_container_name(s) || (s.len() == 64 && is_lower_hex(s))
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `/v1.NN` 접두를 떼고 나머지 경로. 접두가 없으면 그대로.
fn strip_version(path: &str) -> Result<&str, Deny> {
    let Some(rest) = path.strip_prefix("/v") else {
        return Ok(path);
    };
    let (ver, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => return deny("API 판 접두 뒤에 경로가 없다"),
    };
    let minor = ver
        .strip_prefix("1.")
        .filter(|m| !m.is_empty() && m.len() <= 3 && m.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|m| m.parse::<u32>().ok());
    match minor {
        Some(m) if m >= MIN_API_MINOR => Ok(tail),
        Some(_) => deny("API 판이 너무 낮다"),
        None => deny("API 판 접두가 틀렸다"),
    }
}

/// 메서드·경로(질의 빼고)로 호출을 가린다. 허용 목록 밖은 거부.
pub fn classify(method: &str, path: &str) -> Result<Route, Deny> {
    if !path.starts_with('/') || path.contains(['%', '\\']) {
        return deny("경로 꼴이 틀렸다");
    }
    let rest = strip_version(path)?;
    let segs: Vec<&str> = rest[1..].split('/').collect();
    if segs.iter().any(|s| s.is_empty() || s.starts_with('.')) {
        return deny("경로 꼴이 틀렸다");
    }
    let id = |s: &str| -> Result<String, Deny> {
        if is_container_ref(s) {
            Ok(s.to_owned())
        } else {
            deny("판정기 컨테이너 이름·id 꼴이 아니다")
        }
    };
    match (method, segs.as_slice()) {
        ("GET" | "HEAD", ["_ping"]) => Ok(Route::Ping),
        ("GET", ["version"]) => Ok(Route::Version),
        ("POST", ["containers", "create"]) => Ok(Route::Create),
        ("POST", ["containers", c, "attach"]) => Ok(Route::Attach(id(c)?)),
        ("POST", ["containers", c, "start"]) => Ok(Route::Start(id(c)?)),
        ("POST", ["containers", c, "wait"]) => Ok(Route::Wait(id(c)?)),
        ("POST", ["containers", c, "kill"]) => Ok(Route::Kill(id(c)?)),
        ("DELETE", ["containers", c]) => Ok(Route::Delete(id(c)?)),
        ("GET", ["containers", c, "json"]) => Ok(Route::Inspect(id(c)?)),
        _ => deny("허용 목록 밖 호출"),
    }
}

/// 받는 러너 이미지 참조 — 판정기가 쓰는 [`promptlab_shared::coding::RunnerImage::reference`]와 같은 문자열.
pub fn allowed_images() -> Vec<String> {
    RUNNER_IMAGES.iter().map(|i| i.reference()).collect()
}

// ---------- 엄격한 JSON ----------

struct Strict(Value);

impl<'de> serde::Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        let mut folded = HashSet::new();
        while let Some(k) = map.next_key::<String>()? {
            if !k.is_ascii() {
                return Err(de::Error::custom("ASCII가 아닌 키"));
            }
            if !folded.insert(k.to_ascii_lowercase()) {
                return Err(de::Error::custom("겹치는 키(대소문자 무시)"));
            }
            let Strict(v) = map.next_value()?;
            out.insert(k, v);
        }
        Ok(Value::Object(out))
    }
}

/// docker와 다르게 읽힐 수 있는 JSON을 거부하며 읽는다: ASCII가 아닌 키, 대소문자만 다른(또는 같은) 겹친 키, 뒤에 붙은
/// 글자.
pub fn parse_strict(body: &[u8]) -> Result<Value, Deny> {
    serde_json::from_slice::<Strict>(body)
        .map(|s| s.0)
        .map_err(|e| {
            Deny(format!(
                "본문 JSON을 읽지 못했다: {}",
                short(&e.to_string())
            ))
        })
}

fn short(s: &str) -> String {
    s.chars().take(80).collect()
}

// ---------- 값 꼴 ----------

/// 없거나(null) 기본값(false·0·빈 문자열) — 모르는 키가 가질 수 있는 값.
fn is_scalar_default(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_i64() == Some(0) || n.as_u64() == Some(0),
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}
fn null(v: &Value) -> bool {
    v.is_null()
}
fn null_or_false(v: &Value) -> bool {
    matches!(v, Value::Null | Value::Bool(false))
}
fn any_bool(v: &Value) -> bool {
    matches!(v, Value::Null | Value::Bool(_))
}
fn empty_str(v: &Value) -> bool {
    matches!(v, Value::Null) || v.as_str() == Some("")
}
fn zero(v: &Value) -> bool {
    matches!(v, Value::Null) || v.as_i64() == Some(0)
}
fn empty_arr(v: &Value) -> bool {
    matches!(v, Value::Null) || v.as_array().is_some_and(|a| a.is_empty())
}
fn empty_obj(v: &Value) -> bool {
    matches!(v, Value::Null) || v.as_object().is_some_and(|o| o.is_empty())
}

/// docker CLI `RAMInBytes`와 같은 꼴(이진 단위, 소문자·대문자) — `limits::MEMORY`("512m")를 바이트로.
pub fn ram_bytes(s: &str) -> i64 {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len()));
    let n: i64 = num.parse().expect("메모리 상수 숫자");
    let mul: i64 = match unit.to_ascii_lowercase().trim_end_matches('b') {
        "" => 1,
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        u => panic!("메모리 상수 단위 {u}"),
    };
    n * mul
}

/// `--cpus` 상수를 `NanoCpus`로.
pub fn nano_cpus(s: &str) -> i64 {
    (s.parse::<f64>().expect("CPU 상수") * 1e9).round() as i64
}

/// `--tmpfs 경로:옵션` 상수를 `HostConfig.Tmpfs` 꼴로.
pub fn expected_tmpfs() -> Value {
    let mut m = Map::new();
    for t in [limits::TMPFS_WORK, limits::TMPFS_TMP] {
        let (path, opts) = t.split_once(':').expect("tmpfs 상수 꼴");
        m.insert(path.to_owned(), Value::String(opts.to_owned()));
    }
    Value::Object(m)
}

fn is_user(v: &Value) -> bool {
    v.as_str() == Some(limits::USER)
}
fn is_image(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| allowed_images().iter().any(|a| a == s))
}
fn is_network(v: &Value) -> bool {
    v.as_str() == Some(limits::NETWORK)
}
fn is_cap_drop_all(v: &Value) -> bool {
    *v == json!(["ALL"])
}
fn is_true(v: &Value) -> bool {
    *v == Value::Bool(true)
}
fn is_no_new_privileges(v: &Value) -> bool {
    *v == json!(["no-new-privileges"])
}
fn is_tmpfs(v: &Value) -> bool {
    *v == expected_tmpfs()
}
fn is_memory(v: &Value) -> bool {
    v.as_i64() == Some(ram_bytes(limits::MEMORY))
}
fn is_nano_cpus(v: &Value) -> bool {
    v.as_i64() == Some(nano_cpus(limits::CPUS))
}
fn is_pids(v: &Value) -> bool {
    v.as_i64() == Some(i64::from(limits::PIDS))
}
fn is_ulimits(v: &Value) -> bool {
    *v == json!([{ "Name": "nofile", "Soft": limits::NOFILE, "Hard": limits::NOFILE }])
}
fn is_swappiness_default(v: &Value) -> bool {
    v.is_null() || v.as_i64() == Some(-1)
}
fn is_console_size(v: &Value) -> bool {
    v.is_null() || *v == json!([0, 0])
}
fn is_restart_no(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(o) => o.iter().all(|(k, v)| match k.as_str() {
            "Name" => matches!(v.as_str(), Some("" | "no")) || v.is_null(),
            "MaximumRetryCount" => zero(v),
            _ => false,
        }),
        _ => false,
    }
}
fn is_log_default(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(o) => o.iter().all(|(k, v)| match k.as_str() {
            "Type" => empty_str(v),
            "Config" => empty_obj(v),
            _ => false,
        }),
        _ => false,
    }
}
fn is_networking_default(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(o) => o
            .iter()
            .all(|(k, v)| k == "EndpointsConfig" && empty_obj(v)),
        _ => false,
    }
}
fn is_object(v: &Value) -> bool {
    v.is_object()
}
fn is_private_ns(v: &Value) -> bool {
    matches!(v.as_str(), Some("" | "private")) || v.is_null()
}

type Check = fn(&Value) -> bool;

/// (키, 검사, 거부 까닭). 키가 없으면 null로 보고 검사한다 — 꼭 있어야 하는 키(상한 등)는 검사가 null을 거부한다.
type Rule = (&'static str, Check, &'static str);

/// 만들기 본문 맨 위(`container.Config` + `HostConfig` + `NetworkingConfig`).
const CONFIG_RULES: &[Rule] = &[
    ("Hostname", empty_str, "Hostname"),
    ("Domainname", empty_str, "Domainname"),
    ("User", is_user, "User가 65534:65534가 아니다"),
    ("AttachStdin", any_bool, "AttachStdin"),
    ("AttachStdout", any_bool, "AttachStdout"),
    ("AttachStderr", any_bool, "AttachStderr"),
    ("ExposedPorts", empty_obj, "ExposedPorts"),
    ("Tty", null_or_false, "Tty"),
    ("OpenStdin", any_bool, "OpenStdin"),
    ("StdinOnce", any_bool, "StdinOnce"),
    ("Env", empty_arr, "Env"),
    ("Cmd", empty_arr, "Cmd는 이미지 기본이어야 한다"),
    ("Healthcheck", null, "Healthcheck"),
    ("ArgsEscaped", null_or_false, "ArgsEscaped"),
    ("Image", is_image, "허용 목록 밖 이미지"),
    ("Volumes", empty_obj, "Volumes"),
    ("WorkingDir", empty_str, "WorkingDir"),
    (
        "Entrypoint",
        empty_arr,
        "Entrypoint는 이미지 기본이어야 한다",
    ),
    ("NetworkDisabled", any_bool, "NetworkDisabled"),
    ("MacAddress", empty_str, "MacAddress"),
    ("OnBuild", empty_arr, "OnBuild"),
    ("Labels", empty_obj, "Labels"),
    ("StopSignal", empty_str, "StopSignal"),
    ("StopTimeout", null, "StopTimeout"),
    ("Shell", empty_arr, "Shell"),
    ("HostConfig", is_object, "HostConfig가 없다"),
    (
        "NetworkingConfig",
        is_networking_default,
        "NetworkingConfig",
    ),
];

/// `HostConfig`. 근거: [`promptlab_judge::docker_run_args`]의 플래그 → [`limits`] 상수.
const HOST_RULES: &[Rule] = &[
    ("Binds", empty_arr, "바인드 마운트(Binds)"),
    ("ContainerIDFile", empty_str, "ContainerIDFile"),
    ("LogConfig", is_log_default, "LogConfig"),
    ("NetworkMode", is_network, "NetworkMode가 none이 아니다"),
    ("PortBindings", empty_obj, "PortBindings"),
    ("RestartPolicy", is_restart_no, "RestartPolicy"),
    ("AutoRemove", is_true, "AutoRemove(--rm)가 아니다"),
    ("VolumeDriver", empty_str, "VolumeDriver"),
    ("VolumesFrom", empty_arr, "VolumesFrom"),
    ("ConsoleSize", is_console_size, "ConsoleSize"),
    ("Annotations", empty_obj, "Annotations"),
    ("CapAdd", empty_arr, "CapAdd"),
    ("CapDrop", is_cap_drop_all, "CapDrop이 [ALL]이 아니다"),
    ("CgroupnsMode", is_private_ns, "CgroupnsMode"),
    ("Dns", empty_arr, "Dns"),
    ("DnsOptions", empty_arr, "DnsOptions"),
    ("DnsSearch", empty_arr, "DnsSearch"),
    ("ExtraHosts", empty_arr, "ExtraHosts"),
    ("GroupAdd", empty_arr, "GroupAdd"),
    ("IpcMode", is_private_ns, "IpcMode"),
    ("Cgroup", empty_str, "Cgroup"),
    ("Links", empty_arr, "Links"),
    ("OomScoreAdj", zero, "OomScoreAdj"),
    ("PidMode", empty_str, "PidMode"),
    ("Privileged", null_or_false, "Privileged"),
    ("PublishAllPorts", null_or_false, "PublishAllPorts"),
    ("ReadonlyRootfs", is_true, "ReadonlyRootfs가 아니다"),
    (
        "SecurityOpt",
        is_no_new_privileges,
        "SecurityOpt가 [no-new-privileges]가 아니다",
    ),
    ("StorageOpt", empty_obj, "StorageOpt"),
    ("Tmpfs", is_tmpfs, "Tmpfs가 /work·/tmp 상수가 아니다"),
    ("UTSMode", empty_str, "UTSMode"),
    ("UsernsMode", empty_str, "UsernsMode"),
    ("ShmSize", zero, "ShmSize"),
    ("Sysctls", empty_obj, "Sysctls"),
    ("Runtime", empty_str, "Runtime"),
    ("Isolation", empty_str, "Isolation"),
    ("CpuShares", zero, "CpuShares"),
    ("Memory", is_memory, "Memory 상한이 상수와 다르다"),
    ("NanoCpus", is_nano_cpus, "NanoCpus 상한이 상수와 다르다"),
    ("CgroupParent", empty_str, "CgroupParent"),
    ("BlkioWeight", zero, "BlkioWeight"),
    ("BlkioWeightDevice", empty_arr, "BlkioWeightDevice"),
    ("BlkioDeviceReadBps", empty_arr, "BlkioDeviceReadBps"),
    ("BlkioDeviceWriteBps", empty_arr, "BlkioDeviceWriteBps"),
    ("BlkioDeviceReadIOps", empty_arr, "BlkioDeviceReadIOps"),
    ("BlkioDeviceWriteIOps", empty_arr, "BlkioDeviceWriteIOps"),
    ("CpuPeriod", zero, "CpuPeriod"),
    ("CpuQuota", zero, "CpuQuota"),
    ("CpuRealtimePeriod", zero, "CpuRealtimePeriod"),
    ("CpuRealtimeRuntime", zero, "CpuRealtimeRuntime"),
    ("CpusetCpus", empty_str, "CpusetCpus"),
    ("CpusetMems", empty_str, "CpusetMems"),
    ("Devices", empty_arr, "Devices"),
    ("DeviceCgroupRules", empty_arr, "DeviceCgroupRules"),
    ("DeviceRequests", empty_arr, "DeviceRequests"),
    ("KernelMemoryTCP", zero, "KernelMemoryTCP"),
    ("MemoryReservation", zero, "MemoryReservation"),
    (
        "MemorySwap",
        is_memory,
        "MemorySwap이 Memory와 다르다(스왑 없음)",
    ),
    (
        "MemorySwappiness",
        is_swappiness_default,
        "MemorySwappiness",
    ),
    ("OomKillDisable", null_or_false, "OomKillDisable"),
    ("PidsLimit", is_pids, "PidsLimit가 상수와 다르다"),
    ("Ulimits", is_ulimits, "Ulimits가 nofile 상수와 다르다"),
    ("CpuCount", zero, "CpuCount"),
    ("CpuPercent", zero, "CpuPercent"),
    ("IOMaximumIOps", zero, "IOMaximumIOps"),
    ("IOMaximumBandwidth", zero, "IOMaximumBandwidth"),
    ("Mounts", empty_arr, "Mounts(tmpfs는 Tmpfs로만)"),
    // 빈 배열은 「가리지 않음」이라 위험하다 — null(기본 가림)만.
    ("MaskedPaths", null, "MaskedPaths"),
    ("ReadonlyPaths", null, "ReadonlyPaths"),
    ("Init", null_or_false, "Init"),
];

/// 객체를 규칙표로 검사: 알려진 키는 그 검사, 대소문자만 다른 알려진 키는 거부, 모르는 키는 기본값(null·false·0·"")만.
fn check_object(obj: &Map<String, Value>, rules: &[Rule], what: &str) -> Result<(), Deny> {
    for (key, value) in obj {
        match rules.iter().find(|(k, _, _)| k.eq_ignore_ascii_case(key)) {
            Some((k, _, _)) if *k != key => {
                return deny(format!("{what}.{key}: 키 대소문자가 다르다"));
            }
            Some(_) => {}
            None if is_scalar_default(value) => {}
            None => return deny(format!("{what}.{key}: 모르는 키에 기본값이 아닌 값")),
        }
    }
    for (key, check, reason) in rules {
        let v = obj.get(*key).unwrap_or(&Value::Null);
        if !check(v) {
            return deny(format!("{what}: {reason}"));
        }
    }
    Ok(())
}

/// `POST /containers/create` 검사. 성공이면 컨테이너 이름.
///
/// - 질의는 `name=promptlab-run-<32 hex>` 하나(판정기가 늘 이름을 붙인다 — `docker kill <이름>`).
/// - `Content-Type: application/json`, 본문 [`MAX_BODY`] 이하, [`parse_strict`].
/// - 본문은 [`CONFIG_RULES`]·[`HOST_RULES`].
pub fn check_create(
    query: Option<&str>,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<String, Deny> {
    let name = match query.and_then(|q| q.strip_prefix("name=")) {
        Some(n) if is_container_name(n) => n.to_owned(),
        _ => return deny("만들기 질의는 name=promptlab-run-<32 hex> 하나여야 한다"),
    };
    let media = content_type
        .and_then(|c| c.split(';').next())
        .map(str::trim);
    if !media.is_some_and(|m| m.eq_ignore_ascii_case("application/json")) {
        return deny("만들기 본문은 application/json이어야 한다");
    }
    if body.len() > MAX_BODY {
        return deny("만들기 본문이 너무 크다");
    }
    let v = parse_strict(body)?;
    let Value::Object(top) = &v else {
        return deny("만들기 본문이 객체가 아니다");
    };
    check_object(top, CONFIG_RULES, "본문")?;
    let Some(Value::Object(host)) = top.get("HostConfig") else {
        return deny("HostConfig가 없다");
    };
    check_object(host, HOST_RULES, "HostConfig")?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 상수_환산() {
        assert_eq!(ram_bytes("512m"), 536_870_912);
        assert_eq!(nano_cpus("1"), 1_000_000_000);
        assert_eq!(nano_cpus("0.5"), 500_000_000);
        assert_eq!(
            expected_tmpfs(),
            json!({"/work": "size=64m,exec,mode=1777", "/tmp": "size=16m,mode=1777"})
        );
    }
}
