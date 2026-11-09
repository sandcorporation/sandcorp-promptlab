//! 응시 전 Docker 점검 상태 기계(#97): 데몬 확인 → 언어마다 이미지 확인·받기 → 빈 하네스 → 준비/막힘.

use promptlab_app::docker_check::{Action, CheckEvent, DockerCheck, LangState, Stage};
use promptlab_app::localrun::{DockerError, PullProgress};
use promptlab_shared::coding::Language;

#[test]
fn 이미지가_있으면_빈_틀만_돌리고_준비된다() {
    // 허용 언어는 Language::ALL 순서로 맞춘다.
    let mut c = DockerCheck::new(&[Language::Java, Language::Python]);
    assert_eq!(c.stage(), &Stage::Idle);
    assert_eq!(c.next_action(), Action::Probe);
    assert_eq!(c.status_text(), "Docker 확인 중…");
    c.apply(CheckEvent::Probed(Ok("27.3.1".into())));
    assert_eq!(c.next_action(), Action::Inspect(Language::Python));
    c.apply(CheckEvent::Inspected(Language::Python, Ok(true)));
    assert_eq!(c.next_action(), Action::Trial(Language::Python));
    c.apply(CheckEvent::Tried(Language::Python, Ok(())));
    assert_eq!(c.next_action(), Action::Inspect(Language::Java));
    c.apply(CheckEvent::Inspected(Language::Java, Ok(true)));
    assert_eq!(c.next_action(), Action::Trial(Language::Java));
    assert!(!c.ready());
    c.apply(CheckEvent::Tried(Language::Java, Ok(())));
    assert_eq!(c.next_action(), Action::Done);
    assert!(c.ready());
    assert_eq!(c.status_text(), "Docker 27.3.1 · 언어 2개 준비됨");
    assert_eq!(c.next_action(), Action::Done);
}

#[test]
fn 이미지가_없으면_받고_진행을_보인다() {
    let mut c = DockerCheck::new(&[Language::Cpp]);
    c.next_action();
    c.apply(CheckEvent::Probed(Ok("27".into())));
    c.next_action();
    c.apply(CheckEvent::Inspected(Language::Cpp, Ok(false)));
    assert_eq!(c.next_action(), Action::Pull(Language::Cpp));
    let mut p = PullProgress::new();
    p.line("a1b2c3d4e5f6: Pulling fs layer");
    p.line("0f1e2d3c4b5a: Already exists");
    c.apply(CheckEvent::PullProgress(Language::Cpp, p.clone()));
    assert_eq!(c.languages()[0].1, LangState::Pulling(p));
    assert!(
        c.status_text()
            .contains("이미지 받는 중(층 1/2) · 언어 0/1 준비"),
        "{}",
        c.status_text()
    );
    c.apply(CheckEvent::Pulled(Language::Cpp, Ok(())));
    assert_eq!(c.next_action(), Action::Trial(Language::Cpp));
}

#[test]
fn 데몬이_꺼져_있으면_막고_설치_안내를_보이고_다시_확인한다() {
    let mut c = DockerCheck::new(&[Language::Python]);
    c.next_action();
    c.apply(CheckEvent::Probed(Err(DockerError::DaemonDown)));
    assert_eq!(c.next_action(), Action::Done);
    assert!(!c.ready());
    assert_eq!(c.blocker(), Some(&DockerError::DaemonDown));
    assert!(c.needs_install_guide());
    assert_eq!(c.status_text(), DockerError::DaemonDown.to_string());
    c.restart();
    assert_eq!(c.next_action(), Action::Probe);
}

#[test]
fn 받기가_실패하면_막고_다시_확인하면_그_언어부터_다시_본다() {
    let mut c = DockerCheck::new(&[Language::Python, Language::Javascript]);
    c.next_action();
    c.apply(CheckEvent::Probed(Ok("27".into())));
    c.next_action();
    c.apply(CheckEvent::Inspected(Language::Python, Ok(true)));
    c.next_action();
    c.apply(CheckEvent::Tried(Language::Python, Ok(())));
    c.next_action();
    c.apply(CheckEvent::Inspected(Language::Javascript, Ok(false)));
    c.next_action();
    c.apply(CheckEvent::Pulled(
        Language::Javascript,
        Err(DockerError::PullFailed("timeout".into())),
    ));
    assert_eq!(c.next_action(), Action::Done);
    assert!(!c.needs_install_guide());
    assert_eq!(
        c.languages(),
        &[
            (Language::Python, LangState::Ready),
            (Language::Javascript, LangState::Unknown)
        ]
    );
    c.restart();
    assert_eq!(c.next_action(), Action::Probe);
    c.apply(CheckEvent::Probed(Ok("27".into())));
    // 준비된 Python은 건너뛴다.
    assert_eq!(c.next_action(), Action::Inspect(Language::Javascript));
}

#[test]
fn 미설치도_설치_안내를_보이고_권한_문제는_보이지_않는다() {
    let mut c = DockerCheck::new(&[Language::Python]);
    c.next_action();
    c.apply(CheckEvent::Probed(Err(DockerError::NotInstalled)));
    assert!(c.needs_install_guide());
    let mut c = DockerCheck::new(&[Language::Python]);
    c.next_action();
    c.apply(CheckEvent::Probed(Err(DockerError::PermissionDenied)));
    assert!(!c.needs_install_guide());
    assert!(matches!(
        c.stage(),
        Stage::Blocked(DockerError::PermissionDenied)
    ));
}

#[tokio::test]
async fn docker가_없으면_drive가_미설치로_막는다() {
    let mut c = DockerCheck::new(&[Language::Python]);
    let mut seen = Vec::new();
    promptlab_app::docker_check::drive(None, &mut c, |c| seen.push(c.stage().clone())).await;
    assert_eq!(c.blocker(), Some(&DockerError::NotInstalled));
    assert_eq!(seen.first(), Some(&Stage::Probing));
}
