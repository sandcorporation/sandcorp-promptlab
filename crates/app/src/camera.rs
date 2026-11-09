//! 화면 쪽 웹캠(#13): 웹뷰의 getUserMedia로 저해상도 JPEG 한 장씩 받는다(점검·동의 화면은 `ui::check`).
//! 판정·사건 규칙은 lib(`promptlab_app::webcam`) — 여기는 얇게 둔다.
//!
//! 웹캠 영상은 웹뷰 안에만 있고, Rust로 넘어오는 것은 [`capture`]가 찍은 한 장(최대 320px JPEG)뿐이다.

use std::sync::{Arc, Mutex};

use base64::Engine;
use dioxus::prelude::*;
use promptlab_shared::live::{LiveSession, Signal};
use promptlab_vision::{Frame, FrameAnalyzer, Verdict, YuNet};

/// 웹캠을 (처음이면) 켜고 미리보기(`#pl-cam`)에 잇고, 지금 장면을 최대 320px JPEG data URL로 돌려준다.
const CAPTURE_JS: &str = r#"
try {
  if (!window.__plStream || !window.__plStream.active) {
    window.__plStream = await navigator.mediaDevices.getUserMedia({
      video: { width: { ideal: 320 }, height: { ideal: 240 } }, audio: false });
    const hv = document.createElement('video');
    hv.muted = true; hv.playsInline = true; hv.srcObject = window.__plStream;
    await hv.play();
    window.__plVideo = hv;
  }
  const p = document.getElementById('pl-cam');
  if (p && p.srcObject !== window.__plStream) { p.srcObject = window.__plStream; p.play().catch(() => {}); }
  const v = window.__plVideo;
  const w = v.videoWidth || 320, h = v.videoHeight || 240;
  const s = Math.min(1, 320 / Math.max(w, h));
  const c = document.createElement('canvas');
  c.width = Math.round(w * s); c.height = Math.round(h * s);
  c.getContext('2d').drawImage(v, 0, 0, c.width, c.height);
  return c.toDataURL('image/jpeg', 0.7);
} catch (e) {
  return 'error:' + (e && e.name ? e.name + ' ' + e.message : String(e));
}
"#;

/// 웹캠을 끈다(응시가 끝나면).
const STOP_JS: &str = r#"
if (window.__plPc) { try { window.__plPc.close(); } catch (e) {} window.__plPc = null; }
if (window.__plStream) { window.__plStream.getTracks().forEach(t => t.stop()); window.__plStream = null; }
return true;
"#;

/// 지금 장면 한 장(JPEG).
pub async fn capture() -> Result<Vec<u8>, String> {
    let value = document::eval(CAPTURE_JS)
        .await
        .map_err(|e| e.to_string())?;
    let url = value.as_str().unwrap_or_default();
    if let Some(err) = url.strip_prefix("error:") {
        return Err(err.trim().to_owned());
    }
    let data = url
        .strip_prefix("data:image/jpeg;base64,")
        .ok_or("웹캠 사진 형식이 아니다")?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| e.to_string())
}

/// 웹캠을 끈다.
pub fn stop() {
    let _ = document::eval(STOP_JS);
}

/// 실시간 감독(#14) 피어 만들기: 웹캠 스트림의 **영상 트랙만** 붙이고 offer SDP를 돌려준다(음성 없음).
/// TURN relay만 쓴다(`iceTransportPolicy: relay`) — 응시자·감독관이 서로의 IP를 보지 않고, 경로가 늘 같다.
const LIVE_START_JS: &str = r#"
try {
  const cfg = await dioxus.recv();
  if (window.__plPc) { try { window.__plPc.close(); } catch (e) {} }
  if (!window.__plStream || !window.__plStream.active) {
    window.__plStream = await navigator.mediaDevices.getUserMedia({
      video: { width: { ideal: 320 }, height: { ideal: 240 } }, audio: false });
  }
  const pc = new RTCPeerConnection({ iceServers: cfg.ice_servers, iceTransportPolicy: 'relay' });
  window.__plPc = pc; window.__plCands = [];
  pc.onicecandidate = (e) => { if (e.candidate) window.__plCands.push(JSON.stringify(e.candidate.toJSON())); };
  window.__plStream.getVideoTracks().forEach((t) => pc.addTrack(t, window.__plStream));
  const offer = await pc.createOffer();
  await pc.setLocalDescription(offer);
  return 'ok:' + pc.localDescription.sdp;
} catch (e) {
  return 'error:' + (e && e.name ? e.name + ' ' + e.message : String(e));
}
"#;

/// 감독관의 answer·ICE 후보 넣기.
const LIVE_APPLY_JS: &str = r#"
try {
  const m = await dioxus.recv();
  const pc = window.__plPc;
  if (!pc) return 'error:no peer';
  if (m.kind === 'answer') await pc.setRemoteDescription({ type: 'answer', sdp: m.data });
  else if (m.kind === 'ice') await pc.addIceCandidate(JSON.parse(m.data));
  return 'ok';
} catch (e) {
  return 'error:' + String(e);
}
"#;

const LIVE_TAKE_JS: &str = r#"
const c = window.__plCands || []; window.__plCands = []; return c;
"#;

/// 피어만 닫는다(웹캠은 감시 분석이 계속 쓴다).
const LIVE_STOP_JS: &str = r#"
if (window.__plPc) { try { window.__plPc.close(); } catch (e) {} window.__plPc = null; }
window.__plCands = [];
return true;
"#;

/// 웹뷰 RTCPeerConnection으로 송출하는 피어.
pub struct JsPeer;

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

impl promptlab_app::live::Peer for JsPeer {
    async fn start(&mut self, session: &LiveSession) -> Result<String, String> {
        let eval = document::eval(LIVE_START_JS);
        eval.send(serde_json::json!({ "ice_servers": session.ice_servers }))
            .map_err(|e| e.to_string())?;
        js_result(eval.await.map_err(|e| e.to_string())?)
    }

    async fn apply(&mut self, signal: &Signal) -> Result<(), String> {
        let eval = document::eval(LIVE_APPLY_JS);
        eval.send(signal).map_err(|e| e.to_string())?;
        js_result(eval.await.map_err(|e| e.to_string())?).map(|_| ())
    }

    async fn take_candidates(&mut self) -> Vec<String> {
        match document::eval(LIVE_TAKE_JS).await {
            Ok(v) => serde_json::from_value(v).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    async fn stop(&mut self) {
        let _ = document::eval(LIVE_STOP_JS).await;
    }
}

/// 앱 하나에 분석기 하나(처음 쓸 때 올린다).
pub type SharedAnalyzer = Arc<Mutex<Option<YuNet>>>;

/// JPEG 한 장을 분석한다(블로킹 스레드에서).
pub async fn verdict(analyzer: SharedAnalyzer, jpeg: Vec<u8>) -> Result<Verdict, String> {
    tokio::task::spawn_blocking(move || {
        let mut guard = analyzer.lock().map_err(|e| e.to_string())?;
        if guard.is_none() {
            *guard = Some(YuNet::load().map_err(|e| e.to_string())?);
        }
        let frame = Frame::from_jpeg(&jpeg).map_err(|e| e.to_string())?;
        let yunet = guard.as_mut().expect("방금 올렸다");
        Ok(yunet.analyze(&frame).map_err(|e| e.to_string())?.verdict)
    })
    .await
    .map_err(|e| e.to_string())?
}
