//! 응시 전 점검·동의 별도 페이지: 웹캠(시험만) · 장치 점검 · Docker | 이 시험의 방식 · 개인정보. 아래 고정 막대에 동의와
//! 「응시 시작」. 1280px 미만이면 1칸. 웹캠 동의·기준 사진 올리기는 「응시 시작」을 누를 때 한 번에 하고, 이어서 문항을 시작한다.
//! Docker 점검(`promptlab_app::docker_check`)은 시험이면 허용 언어 전부를 받고 빈 틀을 돌려야 시작이 열린다.
//! 개인 연습은 Python만 점검하고 막지 않는다(실행만 못 한다).

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use promptlab_app::auth::open_browser;
use promptlab_app::docker_check::{self, DockerCheck, LangState, Stage};
use promptlab_app::localrun::{INSTALL_URL, LICENSE_NOTE, LocalDocker};
use promptlab_app::webcam::{CameraCheck, Photo, Precheck};
use promptlab_app::workspace::check_single_column;
use promptlab_shared::coding::Language;
use promptlab_shared::webcam::notice;
use promptlab_vision::Verdict;

use super::{FetchState, Route, Target, Ui};
use crate::camera::{self, capture, verdict};
use promptlab_shared::inference::MODELS;

#[component]
pub fn CheckPage(target: Target, checking: bool) -> Element {
    let ui = use_context::<Ui>();
    let (title, back_label, terms, is_exam) = match &target {
        Target::Exam { title, terms, .. } => (title.clone(), "내 시험", terms.clone(), true),
        Target::Practice { problem } => (problem.id.clone(), "개인 연습", None, false),
    };
    let need_camera = terms.is_some();
    // 이미 동의하고 기준 사진을 올린 문항(이어서 응시)은 다시 찍지 않는다.
    let camera_pending = terms
        .as_ref()
        .is_some_and(|t| t.consented_at.is_none() || !t.baseline_uploaded);
    // 고른 AI 모델 파일은 시험만 막는다(문항 시작이 해시를 신고한다). 받기가 끝나면 다시 그릴 때 바뀐다.
    let model_ready = !is_exam || ui.model_ready();
    let languages = match &target {
        Target::Exam { languages, .. } => languages.clone(),
        Target::Practice { .. } => vec![Language::Python],
    };
    let docker = use_signal(|| DockerCheck::new(&languages));
    use_hook(move || run_docker(docker));
    // AI 속도: Vulkan GPU가 보이면 GPU, 아니면 CPU(백엔드만 올린다 — 모델은 올리지 않는다).
    let compute = use_hook(|| promptlab_inference::compute_info().ok());
    let gpu = compute.as_ref().and_then(|c| c.gpu.clone());
    let mut check = use_signal(|| Precheck {
        cpu_threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        model_ready,
        ram_bytes: compute.as_ref().and_then(|c| c.ram_bytes),
        camera: if camera_pending {
            CameraCheck::NotTried
        } else {
            CameraCheck::Seen(Verdict::Ok)
        },
    });
    let mut shot = use_signal(|| None::<Vec<u8>>);
    let mut agreed = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut route = ui.route;

    let try_camera = {
        let analyzer = ui.analyzer.clone();
        move |_| {
            let analyzer = analyzer.clone();
            busy.set(true);
            spawn(async move {
                let camera = match capture().await {
                    Err(e) => CameraCheck::Unavailable(e),
                    Ok(jpeg) => match verdict(analyzer, jpeg.clone()).await {
                        Ok(v) => {
                            shot.set(Some(jpeg));
                            CameraCheck::Seen(v)
                        }
                        Err(e) => CameraCheck::Unavailable(e),
                    },
                };
                check.write().camera = camera;
                busy.set(false);
            });
        }
    };

    let mut c = check.read().clone();
    c.model_ready = !is_exam || ui.model_ready();
    let pick = ui.pick.read().clone();
    let fetch = ui.fetch.read().clone();
    let fetching = matches!(fetch, FetchState::Running { .. });
    let have_model = ui.model_ready();
    let picked_id = pick
        .prefs
        .model
        .clone()
        .unwrap_or_else(|| "auto".to_owned());
    // 웹캠이 필요 없거나 이미 끝난 문항은 `camera`가 `Seen(Ok)`라 모델 파일만 남는다.
    let blockers: Vec<String> = c.blockers();
    let dc = docker.read().clone();
    // 시험은 Docker가 준비돼야 시작한다(로컬 실행 없이 풀게 두지 않는다).
    let docker_ok = !is_exam || dc.ready();
    let ready = blockers.is_empty() && docker_ok && agreed() && !busy() && !checking;
    let single = check_single_column((ui.width)());
    // AI 모델 파일·CPU(+ 시험이면 잠금, 웹캠이 필요하면 웹캠). 모델은 개인 연습도 센다 — 받기 전에 「모두 준비됨」이
    // 뜨지 않게(#131 루프 2, 막는 것은 시험만).
    let camera_ok = !camera_pending || matches!(c.camera, CameraCheck::Seen(Verdict::Ok));
    let devices_ok =
        usize::from(have_model) + 1 + usize::from(is_exam) + usize::from(need_camera && camera_ok);
    let device_total = 2 + usize::from(is_exam) + usize::from(need_camera);
    let devices_all = devices_ok == device_total;

    let start = {
        let ui = ui.clone();
        let target = target.clone();
        let terms = terms.clone();
        move |_| {
            let ui = ui.clone();
            let target = target.clone();
            // 웹캠 동의·기준 사진이 아직이면 올리고, 이어서 문항을 시작한다.
            let upload = if camera_pending {
                terms.as_ref().and(shot.read().clone())
            } else {
                None
            };
            let assignment = match &target {
                Target::Exam { item, .. } => Some(item.assignment_id.clone()),
                Target::Practice { .. } => None,
            };
            busy.set(true);
            error.set(None);
            spawn_forever(async move {
                if let (true, Some(id)) = (camera_pending, assignment) {
                    let result = async {
                        let jpeg = upload.ok_or("기준 사진이 없다")?;
                        let photo = Photo::new(jpeg).map_err(|r| r.message().to_owned())?;
                        ui.api.consent(&id).await.map_err(|e| e.to_string())?;
                        ui.api
                            .put_baseline(&id, &photo)
                            .await
                            .map_err(|e| e.to_string())
                            .map(|_| ())
                    }
                    .await;
                    if let Err(e) = result {
                        error.set(Some(e));
                        busy.set(false);
                        return;
                    }
                    // 동의·기준 사진이 서버에 남았다 — 목록도 고쳐 둔다.
                    let mut assignments = ui.assignments;
                    if let Some(Ok(list)) = assignments.write().as_mut() {
                        for a in list.iter_mut().filter(|a| a.id == id) {
                            if let Some(t) = a.webcam.as_mut() {
                                t.consented_at.get_or_insert(0);
                                t.baseline_uploaded = true;
                            }
                        }
                    }
                }
                camera::stop();
                busy.set(false);
                ui.begin(&target);
            });
        }
    };

    let retention = terms.as_ref().map_or(30, |t| t.retention_days);
    let version = terms
        .as_ref()
        .map(|t| t.notice_version.clone())
        .unwrap_or_default();
    let notice_text = notice(retention);
    rsx! {
        div { class: "page wide check",
            div { class: "page-head",
                h1 { class: "crumbs",
                    button { class: "link", onclick: move |_| { camera::stop(); route.set(Route::Home) }, "{back_label}" }
                    span { class: "sep", "›" }
                    button { class: "link", onclick: move |_| { camera::stop(); route.set(Route::Home) }, "{title}" }
                    span { class: "sep", "›" }
                    "응시 전 점검"
                }
            }
            div { class: if single { "check-grid single" } else { "check-grid" },
                div { class: "check-col",
                    if need_camera {
                        section { class: "card block",
                            h3 { "웹캠" }
                            if camera_pending {
                                p { class: "muted", {webcam_text(&c.camera)} }
                                video { id: "pl-cam", class: "cam", autoplay: true, muted: true, playsinline: true }
                                button { disabled: busy(), onclick: try_camera,
                                    if shot.read().is_some() { "다시 찍기" } else { "웹캠 확인·기준 사진 찍기" }
                                }
                            } else {
                                p { class: "muted", "기준 사진을 이미 올렸다 — 이어서 응시한다." }
                            }
                        }
                    }
                    section { class: "card block",
                        h3 { "장치 점검" }
                        p { class: "muted", if devices_all && blockers.is_empty() { "{device_total}개 모두 준비됨" } else { "{devices_ok}/{device_total}개 준비됨 — 아래를 확인해 주세요" } }
                        div { class: "rows tight",
                            div { class: "row small model-pick",
                                div { class: "grow", strong { "AI 모델" }
                                    span { class: "sub", "{pick.headline()}" }
                                    if let Some(line) = pick.chosen_line() {
                                        span { class: "sub", "{line}" }
                                    }
                                    if let Some(notice) = pick.limited_notice() {
                                        span { class: "sub warn", role: "note", "{notice}" }
                                    }
                                    match &fetch {
                                        FetchState::Running { progress, .. } => {
                                            let text = progress.map_or_else(|| "연결 중".to_owned(), |p| p.text());
                                            rsx! { span { class: "sub", role: "status", "받는 중 {text}" } }
                                        }
                                        FetchState::Failed(e) => rsx! { span { class: "sub fail", role: "alert", "{e}" } },
                                        FetchState::Idle => rsx! {},
                                    }
                                }
                                select { "aria-label": "AI 모델 크기",
                                    disabled: fetching || busy() || checking,
                                    onchange: {
                                        let ui = ui.clone();
                                        move |e: Event<FormData>| {
                                            let v = e.value();
                                            ui.choose_model((v != "auto").then_some(v.as_str()));
                                        }
                                    },
                                    option { value: "auto", selected: picked_id == "auto", "자동(추천: {pick.recommended().model.display_name()})" }
                                    for m in MODELS.iter() {
                                        option { value: "{m.id}", selected: picked_id == m.id, "{pick.option_label(m)}" }
                                    }
                                }
                                if fetching {
                                    span { class: "badge warn", "받는 중" }
                                } else if have_model {
                                    span { class: "badge ok", "받아 둠" }
                                } else {
                                    button { class: "accent",
                                        onclick: {
                                            let ui = ui.clone();
                                            move |_| ui.start_fetch()
                                        },
                                        "받기"
                                    }
                                }
                            }
                            div { class: "row small",
                                div { class: "grow", strong { "AI 속도" }
                                    span { class: "sub",
                                        match &gpu {
                                            Some(name) => rsx! { "GPU — {name}" },
                                            None => rsx! { "CPU — GPU(Vulkan)가 없어 CPU로 답합니다. 답이 느릴 수 있습니다" },
                                        }
                                    }
                                }
                                if gpu.is_some() { span { class: "badge ok", "GPU" } } else { span { class: "badge", "CPU" } }
                            }
                            div { class: "row small",
                                div { class: "grow", strong { "메모리" }
                                    span { class: "sub",
                                        match c.ram_bytes {
                                            Some(b) => rsx! { "{(b + (1 << 29)) >> 30}GB — 권장 16GB 이상" },
                                            None => rsx! { "알 수 없음" },
                                        }
                                    }
                                }
                                if c.ram_low() { span { class: "badge warn", "16GB 미만" } } else if c.ram_bytes.is_some() { span { class: "badge ok", "충분" } }
                            }
                            div { class: "row small",
                                div { class: "grow", strong { "CPU" } span { class: "sub", "스레드 {c.cpu_threads}개" } }
                                if c.cpu_threads >= promptlab_app::webcam::RECOMMENDED_CPU_THREADS { span { class: "badge ok", "충분" } } else { span { class: "badge warn", "느릴 수 있음" } }
                            }
                            if need_camera {
                                div { class: "row small",
                                    div { class: "grow", strong { "웹캠" } span { class: "sub", "얼굴 하나가 정면으로 보여야 합니다" } }
                                    if camera_ok { span { class: "badge ok", "확인됨" } } else { span { class: "badge warn", "확인 전" } }
                                }
                            }
                            if is_exam {
                                div { class: "row small",
                                    div { class: "grow", strong { "전체 화면 잠금" } span { class: "sub", "시작하면 다른 창으로 옮길 수 없습니다" } }
                                    span { class: "badge", "시작하면 켜짐" }
                                }
                            }
                        }
                        for w in c.warnings() { p { class: "infobar warn", role: "status", "{w}" } }
                        for b in blockers.clone() { p { class: "fail", "{b}" } }
                    }
                    DockerCard { check: dc, is_exam, on_retry: move |_| run_docker(docker) }
                }
                div { class: "check-col",
                    section { class: "card block",
                        h3 { "이 시험의 방식" }
                        p { strong { "코드를 쓰고 제출" } }
                        p { class: "muted", "언어를 골라 함수 하나를 완성합니다. 「실행」·「예제 테스트」는 이 PC의 Docker에서 돌고, 제출하면 서버가 숨은 테스트로 채점합니다." }
                        p { strong { "점수는 숨은 테스트 통과율" } }
                        p { class: "muted", "숨은 테스트는 공개되지 않습니다. 공개 예제를 통과해도 숨은 테스트에서 틀릴 수 있습니다." }
                        if is_exam {
                            p { class: "muted", "응시 중에는 창이 전체화면으로 잠깁니다. 다른 창으로 옮기거나(포커스 이탈) 전체화면에서 빠져나가면 과정 로그에 기록되어 시험을 낸 곳이 봅니다. 앱 밖에서 복사한 글은 붙여 넣을 수 없고, 시도하면 기록됩니다. 제한 시간은 「응시 시작」을 누르는 순간부터 흐릅니다(서버 기준)." }
                        }
                    }
                    section { class: "card block",
                        h3 { "개인정보 수집·이용" }
                        if need_camera {
                            pre { class: "notice-text", "{notice_text}" }
                        } else if is_exam {
                            p { class: "muted", "과정 로그(언어 고름·코드 스냅숏·로컬 실행 결과 요약·제출)와 이 앱의 판·모델 파일 해시가 서버로 갑니다. 웹캠·화면 녹화는 없습니다." }
                        } else {
                            p { class: "muted", "감독·화면 기록은 없고, 제출한 코드와 소요 시간만 서버로 갑니다." }
                        }
                    }
                }
            }
            if let Some(e) = error() {
                div { class: "infobar error", role: "alert", "{e}" }
            }
            if checking {
                div { class: "infobar", role: "status", if is_exam { "점검 중: 로컬 모델 해시와 서버의 허용 목록(앱 판·모델 해시) 확인…" } else { "문제를 받는 중…" } }
            }
        }
        div { class: "check-bar",
            label {
                input {
                    r#type: "checkbox",
                    checked: agreed(),
                    onchange: move |e| agreed.set(e.checked()),
                }
                if need_camera {
                    " 개인정보 수집·이용과 시험 방식을 읽었고 동의합니다 (안내문 {version})"
                } else {
                    " 시험 방식과 수집 항목을 읽었고 동의합니다"
                }
            }
            div { class: "bar-actions",
                button { onclick: move |_| { camera::stop(); route.set(Route::Home) }, "취소" }
                button { class: "accent", disabled: !ready, onclick: start,
                    if is_exam { "응시 시작" } else { "연습 시작" }
                }
            }
        }
    }
}

fn webcam_text(camera: &CameraCheck) -> &'static str {
    match camera {
        CameraCheck::NotTried => "아직 확인 전 — 얼굴이 정면으로 보이는 한 장이 기준 사진이 된다",
        CameraCheck::Unavailable(_) => "웹캠을 쓸 수 없음",
        CameraCheck::Seen(Verdict::Ok) => "얼굴 하나 확인 · 기준 사진 준비됨",
        CameraCheck::Seen(_) => "다시 찍어 주세요",
    }
}

/// Docker 점검을 처음부터 돈다(이미 도는 중이면 그대로).
fn run_docker(mut docker: Signal<DockerCheck>) {
    if matches!(docker.read().stage(), Stage::Probing | Stage::Languages) {
        return;
    }
    docker.write().restart();
    spawn(async move {
        let mut local = docker.read().clone();
        let cli = LocalDocker::detect().ok();
        docker_check::drive(cli.as_ref(), &mut local, |c| docker.set(c.clone())).await;
        docker.set(local);
    });
}

fn lang_badge(state: &LangState) -> Element {
    match state {
        LangState::Ready => rsx! { span { class: "badge ok", "준비됨" } },
        LangState::Pulling(p) => rsx! { span { class: "badge", "받는 중 {p.text()}" } },
        LangState::ImageReady => rsx! { span { class: "badge", "빈 틀 실행 중" } },
        LangState::Unknown => rsx! { span { class: "badge", "확인 전" } },
    }
}

/// Docker 카드: 엔진 상태 + 언어별 이미지·빈 틀. 막히면 까닭·설치 안내·다시 확인.
#[component]
fn DockerCard(check: DockerCheck, is_exam: bool, on_retry: EventHandler<()>) -> Element {
    let c = check;
    let blocked = matches!(c.stage(), Stage::Blocked(_));
    let busy = matches!(c.stage(), Stage::Probing | Stage::Languages);
    let lead = if is_exam {
        "「실행」·「예제 테스트」가 이 PC의 Docker에서 돕니다. 시험의 언어마다 실행 이미지를 받고 빈 틀을 한 번 돌려 봅니다."
    } else {
        "「실행」·「예제 테스트」가 이 PC의 Docker에서 돕니다. 없어도 연습은 시작할 수 있지만 로컬 실행은 못 합니다."
    };
    rsx! {
        section { class: "card block",
            h3 { "Docker" }
            p { class: "muted", "{lead}" }
            div { class: "rows tight",
                div { class: "row small",
                    div { class: "grow", strong { "Docker 엔진" } span { class: "sub", role: "status", "{c.status_text()}" } }
                    match c.stage() {
                        Stage::Ready { .. } => rsx! { span { class: "badge ok", "준비됨" } },
                        Stage::Blocked(_) => rsx! { span { class: if is_exam { "badge err" } else { "badge warn" }, "안 됨" } },
                        _ => rsx! { span { class: "badge", "확인 중" } },
                    }
                }
                for (l, state) in c.languages().iter().cloned() {
                    div { class: "row small",
                        div { class: "grow", strong { "{l.display_name()}" } span { class: "sub", "실행 이미지 · 빈 틀 실행" } }
                        {lang_badge(&state)}
                    }
                }
            }
            if let Some(e) = c.blocker() {
                p { class: if is_exam { "infobar error" } else { "infobar warn" }, role: "alert", "{e}" }
            }
            if c.needs_install_guide() {
                p { class: "muted", "{LICENSE_NOTE}" }
                button { onclick: move |_| open_browser(INSTALL_URL), "Docker Desktop 설치 안내 열기" }
            }
            if blocked || !busy {
                button { disabled: busy, onclick: move |_| on_retry.call(()), "다시 확인" }
            }
        }
    }
}
