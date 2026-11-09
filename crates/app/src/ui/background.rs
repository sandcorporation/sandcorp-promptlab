//! 화면과 상관없이 도는 일들: 응시 모드 잠금, 창 신호, 과정 로그 올리기, 실시간 감독 송출, 웹캠 감시, 남은 시간.
//! 응시 중이 아니면 거의 아무것도 하지 않는다. 화면 v2(#74)로 옮기며 동작은 그대로다.

use std::time::{Duration, Instant};

use dioxus::desktop::tao::event::{Event, WindowEvent};
use dioxus::desktop::{use_wry_event_handler, window};
use dioxus::prelude::*;
use promptlab_app::live::{LiveLink, LiveStatus};
use promptlab_app::proclog;
use promptlab_app::session::{ExamItemRef, Phase};
use promptlab_app::webcam::{ANALYZE_EVERY_SECS, Photo, Watch};
use promptlab_shared::live::POLL_MS;

use super::{MAX_PENDING_SNAPSHOTS, PendingSnapshot, Ui, note};
use crate::camera;

pub fn use_background(ui: &Ui) {
    let session = ui.session;
    let assignments = ui.assignments;
    let plog = ui.plog;
    let busy = ui.busy;
    let mut lock = ui.lock;
    let mut watched = ui.watched;
    let mut tick = ui.tick;
    let mut width = ui.width;
    let api = ui.api.clone();
    let book = ui.book.clone();
    let analyzer = ui.analyzer.clone();
    let mut watch = use_signal(|| None::<(ExamItemRef, Watch)>);
    let mut pending_snaps = use_signal(Vec::<PendingSnapshot>::new);

    // 시험 문항을 푸는 동안만 잠근다: 전체화면으로 들어가고, 끝나면(제출·시간 끝) 푼다.
    use_effect(move || {
        let want = {
            let s = session.read();
            matches!(s.phase(), Phase::InProgress) && s.exam_item().is_some()
        };
        let active = lock.peek().is_active();
        if want && !active {
            lock.write().engage();
            window().set_fullscreen(true);
        } else if !want && active {
            lock.write().release();
            window().set_fullscreen(false);
            camera::stop();
        }
    });

    // 창 신호: 포커스를 잃고 되찾음, 전체화면에서 빠져나감(다시 돌린다). 막지 못하니 기록한다.
    use_wry_event_handler(move |event, _| {
        let Event::WindowEvent { event, .. } = event else {
            return;
        };
        match event {
            WindowEvent::Focused(focused) => {
                let noted = lock.write().focus_changed(*focused, proclog::now_ms());
                if let Some(e) = noted {
                    note(plog, e);
                }
            }
            WindowEvent::Resized(_) => {
                width.set(super::window_width());
                let fullscreen = window().fullscreen().is_some();
                let change = lock.write().fullscreen_changed(fullscreen);
                if let Some(e) = change.event {
                    note(plog, e);
                }
                if change.restore {
                    window().set_fullscreen(true);
                }
            }
            _ => {}
        }
    });

    // 과정 로그 올리기: 5초마다 쌓인 것을 올린다(오프라인이면 다음에 다시). 앱을 켤 때 남아 있던 것도 올린다.
    {
        let api = api.clone();
        let book = book.clone();
        use_future(move || {
            let api = api.clone();
            let book = book.clone();
            async move {
                let Ok(book) = book else { return };
                loop {
                    let _ = book.flush_all(&api).await;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        });
    }

    // 실시간 감독(#14): 시험 문항을 푸는 동안 감독관이 「실시간 보기」를 열었는지 폴링하고, 열었을 때만 영상 트랙을 보낸다.
    // 웹캠이 필요 없는 배정(스모크)은 폴링하지 않는다. 보는 중이면 화면 머리에 표시한다.
    {
        let api = api.clone();
        use_future(move || {
            let api = api.clone();
            async move {
                let mut link = LiveLink::new();
                let mut peer = camera::JsPeer;
                let mut current: Option<String> = None;
                loop {
                    let item = {
                        let s = session.read();
                        if matches!(s.phase(), Phase::InProgress) {
                            s.exam_item().cloned()
                        } else {
                            None
                        }
                    };
                    let target = item.map(|i| i.assignment_id).filter(|id| {
                        assignments
                            .read()
                            .as_ref()
                            .and_then(|r| r.as_ref().ok())
                            .and_then(|list| list.iter().find(|a| &a.id == id))
                            .and_then(|a| a.webcam.as_ref())
                            .is_none_or(|t| t.required)
                    });
                    if current != target {
                        if let Some(a) = &current {
                            link.finish(&api, a, &mut peer).await;
                        }
                        current = target;
                    }
                    let streaming = match &current {
                        Some(a) => link
                            .tick(&api, a, &mut peer)
                            .await
                            .map(|st| st == LiveStatus::Streaming)
                            .unwrap_or(link.status() == LiveStatus::Streaming),
                        None => false,
                    };
                    if *watched.peek() != streaming {
                        watched.set(streaming);
                    }
                    // 보는 중이면 시그널을 빨리, 아니면 송출 요청만 느슨하게 본다.
                    let wait = if streaming { POLL_MS } else { 3 * POLL_MS };
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                }
            }
        });
    }

    // 웹캠 감시(#13): 시험 문항을 푸는 동안 3초마다 한 장 분석 → 사건·스냅샷. 웹캠이 필요 없는 배정(스모크)은 건너뛴다.
    {
        let api = api.clone();
        let analyzer = analyzer.clone();
        use_future(move || {
            let api = api.clone();
            let analyzer = analyzer.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(ANALYZE_EVERY_SECS)).await;
                    let item = {
                        let s = session.read();
                        if !matches!(s.phase(), Phase::InProgress) {
                            None
                        } else {
                            s.exam_item().cloned()
                        }
                    };
                    let Some(item) = item else {
                        watch.set(None);
                        continue;
                    };
                    let required = assignments
                        .read()
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .and_then(|list| list.iter().find(|a| a.id == item.assignment_id))
                        .and_then(|a| a.webcam.as_ref())
                        .is_none_or(|t| t.required);
                    if !required {
                        continue;
                    }
                    let now = proclog::now_ms();
                    if watch.peek().as_ref().is_none_or(|(i, _)| *i != item) {
                        let prefix = format!("{}-{}", item.position, now / 1000);
                        watch.set(Some((item.clone(), Watch::new(prefix))));
                    }
                    let shot = camera::capture().await;
                    let planned = match &shot {
                        Err(e) => watch
                            .write()
                            .as_mut()
                            .map(|(_, w)| w.unavailable(e))
                            .unwrap_or_default(),
                        Ok(jpeg) => match camera::verdict(analyzer.clone(), jpeg.clone()).await {
                            Ok(v) => watch
                                .write()
                                .as_mut()
                                .map(|(_, w)| w.frame(now, v))
                                .unwrap_or_default(),
                            Err(e) => watch
                                .write()
                                .as_mut()
                                .map(|(_, w)| w.unavailable(&e))
                                .unwrap_or_default(),
                        },
                    };
                    for p in planned {
                        note(plog, p.event);
                        if let (Some((client_ref, reason)), Ok(jpeg)) = (p.snapshot, &shot) {
                            match Photo::new(jpeg.clone()) {
                                Ok(photo) => {
                                    let mut q = pending_snaps.write();
                                    if q.len() < MAX_PENDING_SNAPSHOTS {
                                        q.push(PendingSnapshot {
                                            item: item.clone(),
                                            client_ref,
                                            reason,
                                            at_ms: now,
                                            photo,
                                        });
                                    }
                                }
                                Err(r) => eprintln!("스냅샷을 올릴 수 없는 사진: {}", r.message()),
                            }
                        }
                    }
                    // 쌓인 스냅샷을 올린다(오프라인이면 다음 주기에 다시).
                    let queue = pending_snaps.peek().clone();
                    let mut left = Vec::new();
                    for snap in queue {
                        let sent = api
                            .upload_snapshot(
                                &snap.item.assignment_id,
                                snap.item.position,
                                snap.reason,
                                &snap.client_ref,
                                snap.at_ms,
                                &snap.photo,
                            )
                            .await;
                        match sent {
                            Ok(_) => {}
                            // 서버가 거절한 것(상한·형식)은 다시 보내지 않는다.
                            Err(promptlab_app::client::ClientError::Rejected {
                                status,
                                message,
                            }) if status != 401 => {
                                eprintln!("스냅샷 거절({status}): {message}");
                            }
                            Err(_) => left.push(snap),
                        }
                    }
                    pending_snaps.set(left);
                }
            }
        });
    }

    // 남은 시간 다시 그리기와, 시험 문항의 시간이 다 되면 바로 제출.
    {
        let ui = ui.clone();
        use_future(move || {
            let ui = ui.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    tick += 1;
                    // 코드가 바뀌었으면 5초마다 스냅숏(과정 로그, 시험 문항만).
                    ui.snapshot(false);
                    let due = {
                        let s = session.read();
                        matches!(s.phase(), Phase::InProgress) && s.time_up(Instant::now())
                    };
                    if due && !*busy.read() {
                        ui.submit_now();
                    }
                }
            }
        });
    }
}
