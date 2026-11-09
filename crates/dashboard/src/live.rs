//! 실시간 감독(#14): 채용 시험의 응시 가능 시간 열고 닫기, 진행 중 응시자의 웹캠 실시간 보기(WebRTC).
//! 감독 권한(관리자·감독관)만 — 서버가 강제한다. 규칙은 [`promptlab_shared::live`].
//!
//! 실시간 보기: 「실시간 보기」를 누르면 세션을 열고(앱이 다음 폴링에서 송출을 시작한다) 서버를 폴링해 앱의 offer에
//! answer를 보낸다. 칸을 닫으면(또는 화면을 떠나면) 세션을 닫아 앱이 송출을 멈춘다. 영상만 오고(음성 없음),
//! 녹화하지 않는다 — 영상은 이 브라우저의 `<video>`에만 흐른다.

use dioxus::prelude::*;
use promptlab_shared::live::{ExamWindow, ExamWindowView, POLL_MS, PostSignal, SignalKind};

use crate::{api, notice};

// ───────────────────────── 응시 가능 시간 ─────────────────────────

/// 한국 시각 `YYYY-MM-DDTHH:MM`(`datetime-local` 값)을 유닉스 초로.
pub(crate) fn parse_kst(value: &str) -> Option<i64> {
    let (date, time) = value.trim().split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (hh, mm) = (t.next()??, t.next()??);
    if !(1..=12).contains(&m)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hh)
        || !(0..60).contains(&mm)
    {
        return None;
    }
    // Howard Hinnant의 days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 - 9 * 3600)
}

/// 유닉스 초를 `datetime-local` 값(한국 시각)으로.
pub(crate) fn kst_input(at: i64) -> String {
    let secs = at + 9 * 3600;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

fn describe(v: &ExamWindowView) -> String {
    let range = match (v.window.starts_at, v.window.ends_at) {
        (None, None) => String::new(),
        (s, e) => format!(
            " · 범위 {} ~ {}",
            s.map(|s| kst_input(s).replace('T', " "))
                .unwrap_or_else(|| "제한 없음".into()),
            e.map(|e| kst_input(e).replace('T', " "))
                .unwrap_or_else(|| "제한 없음".into()),
        ),
    };
    let state = if v.window.open { "열림" } else { "닫힘" };
    format!("감독관 설정: {state}{range} (한국 시각)")
}

/// 채용 시험의 응시 가능 시간 칸(감독 권한).
#[component]
pub fn WindowPanel(org_id: String, exam_id: String) -> Element {
    let ids = (org_id.clone(), exam_id.clone());
    let mut view = use_resource(move || {
        let (org_id, exam_id) = ids.clone();
        async move { api::exam_window(&org_id, &exam_id).await }
    });
    let mut starts = use_signal(String::new);
    let mut ends = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);

    let save = {
        let (org_id, exam_id) = (org_id.clone(), exam_id.clone());
        move |window: ExamWindow| {
            let (org_id, exam_id) = (org_id.clone(), exam_id.clone());
            error.set(None);
            saving.set(true);
            spawn(async move {
                match api::set_exam_window(&org_id, &exam_id, &window).await {
                    Ok(_) => view.restart(),
                    Err(e) => error.set(Some(e.to_string())),
                }
                saving.set(false);
            });
        }
    };

    // 처음 그릴 때 입력 칸을 저장된 범위로 채운다.
    use_effect(move || {
        if let Some(Ok(v)) = &*view.read() {
            starts.set(v.window.starts_at.map(kst_input).unwrap_or_default());
            ends.set(v.window.ends_at.map(kst_input).unwrap_or_default());
        }
    });
    let current = match &*view.read() {
        None => {
            return rsx! { section { class: "sc-panel sc-panel-section window", p { class: "muted", "응시 가능 시간을 불러오는 중…" } } };
        }
        Some(Err(e)) => return notice(e.to_string()),
        Some(Ok(v)) => *v,
    };
    let toggle = {
        let mut save = save.clone();
        move |_| {
            save(ExamWindow {
                open: !current.window.open,
                ..current.window
            })
        }
    };
    let save_range = {
        let mut save = save.clone();
        move |_| {
            let parse = |v: String| -> Result<Option<i64>, String> {
                if v.trim().is_empty() {
                    Ok(None)
                } else {
                    parse_kst(&v)
                        .map(Some)
                        .ok_or_else(|| format!("시각 형식이 틀렸습니다: {v}"))
                }
            };
            match (parse(starts()), parse(ends())) {
                (Ok(starts_at), Ok(ends_at)) => save(ExamWindow {
                    open: current.window.open,
                    starts_at,
                    ends_at,
                }),
                (Err(e), _) | (_, Err(e)) => error.set(Some(e)),
            }
        }
    };
    let mut clear_range = save.clone();
    rsx! {
        section { class: "sc-panel sc-panel-section window",
            div { class: "row spread",
                h3 { "응시 가능 시간" }
                span { class: if current.open_now { "sc-badge sc-badge--success" } else { "sc-badge sc-badge--danger" },
                    if current.open_now { "지금 응시 가능" } else { "지금 응시 불가" }
                }
            }
            p { class: "muted small", "{describe(&current)}" }
            p { class: "muted small",
                "닫혀 있거나 범위 밖이면 응시자가 새 문항을 시작하지 못합니다. 이미 시작한 문항은 제한 시간까지 그대로 진행됩니다."
            }
            div { class: "row",
                button {
                    class: if current.window.open { "sc-btn sc-btn--secondary sc-btn--sm" } else { "sc-btn sc-btn--accent sc-btn--sm" },
                    disabled: saving(),
                    onclick: toggle,
                    if current.window.open { "응시 닫기" } else { "응시 열기" }
                }
            }
            div { class: "row range",
                label { class: "field inline",
                    span { class: "muted small", "시작" }
                    input {
                        class: "sc-input",
                        r#type: "datetime-local",
                        value: "{starts}",
                        oninput: move |e| starts.set(e.value()),
                    }
                }
                label { class: "field inline",
                    span { class: "muted small", "끝" }
                    input {
                        class: "sc-input",
                        r#type: "datetime-local",
                        value: "{ends}",
                        oninput: move |e| ends.set(e.value()),
                    }
                }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", disabled: saving(), onclick: save_range, "범위 저장" }
                if current.window.starts_at.is_some() || current.window.ends_at.is_some() {
                    button {
                        class: "sc-btn sc-btn--secondary sc-btn--sm",
                        disabled: saving(),
                        onclick: move |_| clear_range(ExamWindow { open: current.window.open, starts_at: None, ends_at: None }),
                        "범위 지우기"
                    }
                }
            }
            if let Some(e) = error() {
                p { class: "sc-alert sc-alert--error", role: "alert", "{e}" }
            }
        }
    }
}

// ───────────────────────── 실시간 보기 ─────────────────────────

/// 감독관 피어 만들기: 받기만 한다(영상). 앱은 relay만 쓰므로 응시자 IP는 보이지 않는다. 감독관 쪽은 모든 후보를 써서
/// TURN 안에서 relay끼리 되돌아 나가는(hairpin) 경로를 피한다.
const VIEW_START_JS: &str = r#"
try {
  const cfg = await dioxus.recv();
  if (window.__plvPc) { try { window.__plvPc.close(); } catch (e) {} }
  const pc = new RTCPeerConnection({ iceServers: cfg.ice_servers });
  window.__plvPc = pc; window.__plvCands = []; window.__plvState = 'new';
  pc.onicecandidate = (e) => { if (e.candidate) window.__plvCands.push(JSON.stringify(e.candidate.toJSON())); };
  pc.onconnectionstatechange = () => { window.__plvState = pc.connectionState; };
  pc.ontrack = (e) => {
    const v = document.getElementById('pl-live');
    if (v) { v.srcObject = (e.streams && e.streams[0]) || new MediaStream([e.track]); v.play().catch(() => {}); }
  };
  return 'ok';
} catch (e) {
  return 'error:' + (e && e.name ? e.name + ' ' + e.message : String(e));
}
"#;

/// 앱의 offer → answer SDP, ICE 후보 → 넣기.
const VIEW_APPLY_JS: &str = r#"
try {
  const m = await dioxus.recv();
  const pc = window.__plvPc;
  if (!pc) return 'error:no peer';
  if (m.kind === 'offer') {
    await pc.setRemoteDescription({ type: 'offer', sdp: m.data });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    return 'ok:' + pc.localDescription.sdp;
  }
  if (m.kind === 'ice') await pc.addIceCandidate(JSON.parse(m.data));
  return 'ok';
} catch (e) {
  return 'error:' + String(e);
}
"#;

const VIEW_TAKE_JS: &str = r#"
const c = window.__plvCands || []; window.__plvCands = [];
return { candidates: c, state: window.__plvState || 'new' };
"#;

const VIEW_STOP_JS: &str = r#"
if (window.__plvPc) { try { window.__plvPc.close(); } catch (e) {} window.__plvPc = null; }
const v = document.getElementById('pl-live'); if (v) v.srcObject = null;
return true;
"#;

fn js_result(value: serde_json::Value) -> Result<String, String> {
    let text = value.as_str().unwrap_or_default();
    match text.strip_prefix("ok") {
        Some(rest) => Ok(rest.strip_prefix(':').unwrap_or(rest).to_owned()),
        None => Err(text
            .strip_prefix("error:")
            .unwrap_or(text)
            .trim()
            .to_owned()),
    }
}

async fn eval_with(js: &str, input: serde_json::Value) -> Result<String, String> {
    let eval = document::eval(js);
    eval.send(input).map_err(|e| e.to_string())?;
    js_result(eval.await.map_err(|e| e.to_string())?)
}

#[derive(Debug, Clone, PartialEq)]
enum Stage {
    Opening,
    Waiting,
    Connecting,
    Live,
    Ended(String),
}

impl Stage {
    fn label(&self) -> String {
        match self {
            Stage::Opening => "여는 중…".into(),
            Stage::Waiting => {
                "응시자 앱이 연결하기를 기다리는 중… (문항을 푸는 중일 때만 송출됩니다)".into()
            }
            Stage::Connecting => "연결하는 중…".into(),
            Stage::Live => "실시간으로 보는 중 — 녹화하지 않으며 음성은 없습니다".into(),
            Stage::Ended(why) => why.clone(),
        }
    }
}

/// 실시간 보기 칸. 닫으면(`on_close`, 또는 화면을 떠나면) 세션을 닫아 앱이 송출을 멈춘다.
#[component]
pub fn LivePanel(
    org_id: String,
    exam_id: String,
    invite_id: String,
    candidate: String,
    on_close: EventHandler<()>,
) -> Element {
    let mut stage = use_signal(|| Stage::Opening);
    let mut session = use_signal(|| None::<String>);
    let ids = (org_id.clone(), exam_id.clone(), invite_id.clone());
    use_future(move || {
        let (org_id, exam_id, invite_id) = ids.clone();
        async move {
            let opened = match api::open_live(&org_id, &exam_id, &invite_id).await {
                Ok(s) => s,
                Err(e) => return stage.set(Stage::Ended(e.to_string())),
            };
            let sid = opened.session_id.clone();
            session.set(Some(sid.clone()));
            if let Err(e) = eval_with(
                VIEW_START_JS,
                serde_json::json!({ "ice_servers": opened.ice_servers }),
            )
            .await
            {
                return stage.set(Stage::Ended(format!(
                    "이 브라우저에서 WebRTC를 쓸 수 없습니다: {e}"
                )));
            }
            stage.set(Stage::Waiting);
            let mut after = 0;
            loop {
                let batch =
                    match api::live_signals(&org_id, &exam_id, &invite_id, &sid, after).await {
                        Ok(b) => b,
                        // 잠깐 끊긴 것일 수 있다 — 다음 폴링에서 다시.
                        Err(_) => {
                            api::sleep_ms(POLL_MS as u32).await;
                            continue;
                        }
                    };
                if !batch.open {
                    let _ = document::eval(VIEW_STOP_JS).await;
                    return stage.set(Stage::Ended(
                        "실시간 보기가 끝났습니다(응시자 쪽 종료 또는 시간 초과).".into(),
                    ));
                }
                for signal in batch.signals {
                    after = after.max(signal.seq);
                    match signal.kind {
                        SignalKind::Offer => {
                            stage.set(Stage::Connecting);
                            match eval_with(
                                VIEW_APPLY_JS,
                                serde_json::to_value(&signal).unwrap_or_default(),
                            )
                            .await
                            {
                                Ok(answer) => {
                                    let _ = api::post_live_signal(
                                        &org_id,
                                        &exam_id,
                                        &invite_id,
                                        &sid,
                                        &PostSignal {
                                            kind: SignalKind::Answer,
                                            data: answer,
                                        },
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    stage.set(Stage::Ended(format!("연결하지 못했습니다: {e}")))
                                }
                            }
                        }
                        SignalKind::Ice => {
                            let _ = eval_with(
                                VIEW_APPLY_JS,
                                serde_json::to_value(&signal).unwrap_or_default(),
                            )
                            .await;
                        }
                        SignalKind::Bye => {
                            let _ = document::eval(VIEW_STOP_JS).await;
                            let why = if signal.data.is_empty() {
                                "응시자 앱이 송출을 끝냈습니다.".to_owned()
                            } else {
                                format!("응시자 앱이 송출하지 못했습니다: {}", signal.data)
                            };
                            return stage.set(Stage::Ended(why));
                        }
                        SignalKind::Answer => {}
                    }
                }
                if let Ok(v) = document::eval(VIEW_TAKE_JS).await {
                    if v["state"] == "connected" {
                        stage.set(Stage::Live);
                    }
                    let candidates: Vec<String> =
                        serde_json::from_value(v["candidates"].clone()).unwrap_or_default();
                    for data in candidates {
                        let _ = api::post_live_signal(
                            &org_id,
                            &exam_id,
                            &invite_id,
                            &sid,
                            &PostSignal {
                                kind: SignalKind::Ice,
                                data,
                            },
                        )
                        .await;
                    }
                }
                api::sleep_ms(POLL_MS as u32).await;
            }
        }
    });
    // 칸이 사라지면(닫기·다른 응시자 선택·화면 이동) 세션을 닫는다.
    let close_ids = (org_id.clone(), exam_id.clone(), invite_id.clone());
    use_drop(move || {
        let (org_id, exam_id, invite_id) = close_ids.clone();
        let sid = session.peek().clone();
        dioxus::core::spawn_forever(async move {
            let _ = document::eval(VIEW_STOP_JS).await;
            if let Some(sid) = sid {
                let _ = api::close_live(&org_id, &exam_id, &invite_id, &sid).await;
            }
        });
    });
    let ended = matches!(*stage.read(), Stage::Ended(_));
    rsx! {
        section { class: "sc-panel sc-panel-section live",
            div { class: "row spread",
                h3 { "실시간 보기 · {candidate}" }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| on_close.call(()), "닫기" }
            }
            p { class: if ended { "sc-alert sc-alert--error" } else { "muted small" }, "{stage.read().label()}" }
            video {
                id: "pl-live",
                class: "live-video",
                autoplay: true,
                muted: true,
                playsinline: true,
            }
            p { class: "muted small",
                "응시자 화면에 「감독관이 실시간으로 보는 중」이 표시됩니다. 닫으면 송출이 멈춥니다."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 한국_시각_입력값을_유닉스_초로_오가며_바꾼다() {
        // 2026-01-01 00:00 KST = 2025-12-31 15:00 UTC = 1767193200.
        assert_eq!(parse_kst("2026-01-01T00:00"), Some(1_767_193_200));
        assert_eq!(kst_input(1_767_193_200), "2026-01-01T00:00");
        let t = parse_kst("2026-11-06T14:30").unwrap();
        assert_eq!(kst_input(t), "2026-11-06T14:30");
        assert_eq!(parse_kst("2026-13-01T00:00"), None);
        assert_eq!(parse_kst("아무거나"), None);
    }
}
