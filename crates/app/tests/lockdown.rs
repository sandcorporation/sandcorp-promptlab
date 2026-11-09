//! 응시 모드 잠금(#12): 창 신호(포커스·전체화면)를 과정 로그 사건으로 바꾸는 규칙, 문항 시작 신고값.

use promptlab_app::exam::client_integrity;
use promptlab_app::lockdown::Lockdown;
use promptlab_shared::inference::LARGEST_MODEL as MODEL;
use promptlab_shared::integrity::check;
use promptlab_shared::log::LogEvent;

#[test]
fn 잠그기_전과_푼_뒤에는_창_신호가_사건이_아니다() {
    let mut l = Lockdown::new();
    assert_eq!(l.focus_changed(false, 1_000), None);
    assert_eq!(l.fullscreen_changed(false).event, None);
    assert!(!l.fullscreen_changed(false).restore);
    l.engage();
    l.release();
    assert!(!l.is_active());
    assert_eq!(l.focus_changed(false, 2_000), None);
}

#[test]
fn 포커스를_잃고_되찾으면_이탈과_비운_시간을_남긴다() {
    let mut l = Lockdown::new();
    l.engage();
    assert_eq!(
        l.focus_changed(true, 0),
        None,
        "처음부터 포커스면 사건 없음"
    );
    assert_eq!(l.focus_changed(false, 10_000), Some(LogEvent::FocusLost));
    // 잃은 채로 또 잃었다는 신호(창마다 오는 중복)는 한 번만.
    assert_eq!(l.focus_changed(false, 11_000), None);
    assert_eq!(
        l.focus_changed(true, 25_500),
        Some(LogEvent::FocusReturned { away_ms: 15_500 })
    );
    assert_eq!(l.focus_changed(true, 26_000), None);
}

#[test]
fn 전체화면에서_빠져나가면_기록하고_다시_전체화면으로_돌린다() {
    let mut l = Lockdown::new();
    l.engage();
    // 전체화면에 들어가기 전의 크기 변화: 사건은 없고 전체화면으로 돌린다.
    let c = l.fullscreen_changed(false);
    assert_eq!(c.event, None);
    assert!(c.restore);
    let c = l.fullscreen_changed(true);
    assert_eq!(c.event, None);
    assert!(!c.restore);
    let c = l.fullscreen_changed(false);
    assert_eq!(c.event, Some(LogEvent::FullscreenExited));
    assert!(c.restore);
    // 잠금을 풀면 전체화면이 풀려도 사건이 아니다.
    l.fullscreen_changed(true);
    l.release();
    let c = l.fullscreen_changed(false);
    assert_eq!(c.event, None);
    assert!(!c.restore);
}

#[test]
fn 문항_시작_신고값은_이_앱_판과_잰_모델_해시이고_허용_목록을_통과한다() {
    let report = client_integrity(MODEL.sha256);
    assert_eq!(report.app_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(check(Some(&report)), Ok(()));
    assert!(check(Some(&client_integrity("deadbeef"))).is_err());
}
