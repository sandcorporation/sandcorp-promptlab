//! 응시 화면(#74·#97)의 lib 로직: 배치 기준폭·단축키·숫자·점수 글자.

use promptlab_app::workspace::{
    DEFAULT_AI_WIDTH, DEFAULT_FOLDED_AI_WIDTH, DEFAULT_PROBLEM_WIDTH, FOLDED_RAIL_WIDTH,
    HARD_MIN_AI_WIDTH, KeyInput, LAYOUT_FILE, LayoutPrefs, MIN_AI_WIDTH, MIN_PROBLEM_WIDTH,
    MIN_WINDOW_WIDTH, MIN_WORK_WIDTH, PaneLayout, PaneWidths, SHORTCUT_HINT, Shortcut,
    check_single_column, drag_ai, drag_problem, fit, nav_collapsed, shortcut, thousands,
};

fn key(key: &str, ctrl: bool, shift: bool) -> KeyInput<'_> {
    KeyInput {
        key,
        ctrl,
        shift,
        alt: false,
        meta: false,
    }
}

#[test]
fn 폭_1440_이상이면_3칸_미만이면_문제_칸이_접힌다() {
    assert_eq!(PaneLayout::for_width(1920), PaneLayout::Three);
    assert_eq!(PaneLayout::for_width(1440), PaneLayout::Three);
    assert_eq!(PaneLayout::for_width(1439), PaneLayout::Folded);
    assert_eq!(PaneLayout::for_width(1280), PaneLayout::Folded);
}

#[test]
fn 점검_페이지는_1280_미만이면_1칸이고_내비는_1280_이하에서_접힌다() {
    assert!(!check_single_column(1280));
    assert!(check_single_column(1279));
    assert!(nav_collapsed(1280));
    assert!(!nav_collapsed(1281));
}

#[test]
fn ctrl_enter는_예제_테스트_ctrl_shift_enter는_실행이다() {
    assert_eq!(
        shortcut(key("Enter", true, false)),
        Some(Shortcut::RunExamples)
    );
    assert_eq!(shortcut(key("Enter", true, true)), Some(Shortcut::RunInput));
    // macOS Cmd도 Ctrl처럼.
    let cmd = KeyInput {
        meta: true,
        ..key("Enter", false, false)
    };
    assert_eq!(shortcut(cmd), Some(Shortcut::RunExamples));
    // 그냥 Enter·Shift+Enter는 줄바꿈(편집기 기본 동작), 다른 키는 아무것도 아니다.
    assert_eq!(shortcut(key("Enter", false, false)), None);
    assert_eq!(shortcut(key("Enter", false, true)), None);
    assert_eq!(shortcut(key("t", true, false)), None);
    assert_eq!(shortcut(key("1", true, false)), None);
    let alt = KeyInput {
        alt: true,
        ..key("Enter", true, false)
    };
    assert_eq!(shortcut(alt), None);
    assert!(SHORTCUT_HINT.contains("Ctrl+Enter"));
}

#[test]
fn 천_단위_쉼표() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(1240), "1,240");
    assert_eq!(thousands(1234567), "1,234,567");
}

#[test]
fn 점수_글자() {
    use promptlab_app::workspace::score_text;
    assert_eq!(score_text(78.0), "78");
    assert_eq!(score_text(72.5), "72.5");
    assert_eq!(score_text(42.499), "42.5");
}

#[test]
fn 대화_평가가_남은_동안만_상태를_계속_받는다() {
    use promptlab_app::session::Phase;
    use promptlab_app::workspace::should_poll;
    use promptlab_shared::coding::Language;
    use promptlab_shared::evaluation::{EvaluationStatus, ItemScore, ScoreBasis};
    use promptlab_shared::grading::GradeReport;
    let report = GradeReport::from_cases("p", Language::Python, &[true]);
    let score = |basis| ItemScore {
        final_score: None,
        hidden_score: Some(100.0),
        evaluation_score: None,
        weight: 50,
        basis,
        formula: String::new(),
        evaluation_status: EvaluationStatus::Queued,
        evaluation: None,
    };
    let graded = |score| Phase::Graded {
        submission_id: "s".into(),
        report: report.clone(),
        score,
    };
    assert!(should_poll(&Phase::Grading {
        submission_id: "s".into()
    }));
    assert!(should_poll(&graded(Some(score(ScoreBasis::Pending)))));
    assert!(!should_poll(&graded(Some(score(ScoreBasis::Weighted)))));
    assert!(!should_poll(&graded(None)));
    assert!(!should_poll(&Phase::Completed));
}

#[test]
fn ai_대화는_맨_아래_40px_안이면_따라가고_위로_올려_읽으면_새_메시지만_알린다() {
    use promptlab_app::workspace::{FOLLOW_BOTTOM_PX, ScrollFollow, near_bottom};
    assert_eq!(FOLLOW_BOTTOM_PX, 40.0);
    // 높이 1000, 보이는 400: 맨 아래 = top 600.
    assert!(near_bottom(600.0, 1000.0, 400.0));
    assert!(near_bottom(560.0, 1000.0, 400.0));
    assert!(!near_bottom(559.0, 1000.0, 400.0));

    let mut f = ScrollFollow::default();
    assert!(f.following());
    assert!(f.on_content(), "처음에는 따라간다");
    assert!(!f.unseen());
    // 위로 올려 읽는 중: 따라가지 않고 새 메시지 표시.
    f.on_scroll(false);
    assert!(!f.on_content());
    assert!(f.unseen());
    assert!(!f.on_content());
    // 스스로 맨 아래로 내려오면 다시 따라가고 표시가 꺼진다.
    f.on_scroll(true);
    assert!(!f.unseen());
    assert!(f.on_content());
    // 「새 메시지 ↓」·보내기는 바로 따라간다.
    f.on_scroll(false);
    f.on_content();
    f.jump();
    assert!(f.following() && !f.unseen());
}

const THREE: PaneLayout = PaneLayout::Three;
const FOLDED: PaneLayout = PaneLayout::Folded;

fn w(problem: u32, ai: u32) -> PaneWidths {
    PaneWidths { problem, ai }
}

#[test]
fn 창에_들어가면_폭을_그대로_둔다() {
    assert_eq!(fit(1920, THREE, w(380, 360)), w(380, 360));
    assert_eq!(w(380, 360).work(1920), 1180);
}

#[test]
fn 최소_폭_아래_값은_최소_폭으로_올린다() {
    assert_eq!(
        fit(1920, THREE, w(10, 10)),
        w(MIN_PROBLEM_WIDTH, MIN_AI_WIDTH)
    );
}

#[test]
fn 창이_줄면_편집기_최소_폭을_남기고_남는_몫_비율로_줄인다() {
    let r = fit(1440, THREE, w(600, 600));
    assert_eq!(r.work(1440), MIN_WORK_WIDTH);
    assert_eq!(r.problem + r.ai, 1440 - MIN_WORK_WIDTH);
    let r = fit(1440, THREE, w(500, 700));
    assert_eq!(r.problem + r.ai + MIN_WORK_WIDTH, 1440);
    assert!(r.problem >= MIN_PROBLEM_WIDTH && r.ai >= MIN_AI_WIDTH);
    // 남는 몫(220 : 400) 비율: 문제가 더 적게 줄어든다.
    assert!(500 - r.problem < 700 - r.ai);
}

#[test]
fn 창이_최소_합보다_좁아도_칸은_최소_폭_아래로_안_간다() {
    let r = fit(900, THREE, w(380, 360));
    assert_eq!(r, w(MIN_PROBLEM_WIDTH, MIN_AI_WIDTH));
}

#[test]
fn 접힘_모드는_문제_칸이_막대_폭이고_ai만_맞춘다() {
    assert_eq!(fit(1300, FOLDED, w(380, 320)), w(FOLDED_RAIL_WIDTH, 320));
    assert_eq!(
        fit(1300, FOLDED, w(0, 5000)).ai,
        1300 - FOLDED_RAIL_WIDTH - MIN_WORK_WIDTH
    );
    assert_eq!(fit(700, FOLDED, w(0, 100)).ai, HARD_MIN_AI_WIDTH);
}

#[test]
fn 창_폭_816_1024_1280에서_편집기는_도구_막대가_들어가는_최소_폭_이상이다() {
    for total in [816, 1024, 1280] {
        for ai in [0, 240, 300, 320, 900] {
            let r = fit(total, FOLDED, w(0, ai));
            assert!(r.work(total) >= MIN_WORK_WIDTH, "{total}/{ai}: {r:?}");
            assert!(r.ai >= HARD_MIN_AI_WIDTH);
            let d = drag_ai(total, FOLDED, r, i64::from(total) - 10);
            assert!(d.work(total) >= MIN_WORK_WIDTH, "끌기 {total}: {d:?}");
        }
    }
    // 1024 이상이면 AI는 최소 폭 300을 지킨다. 816은 288까지 줄어든다.
    assert_eq!(fit(1024, FOLDED, w(0, 0)).ai, MIN_AI_WIDTH);
    assert_eq!(
        fit(816, FOLDED, w(0, 320)).ai,
        816 - FOLDED_RAIL_WIDTH - MIN_WORK_WIDTH
    );
    const { assert!(MIN_WINDOW_WIDTH >= FOLDED_RAIL_WIDTH + MIN_WORK_WIDTH + MIN_AI_WIDTH) };
}

#[test]
fn 문제_경계를_끌면_최소와_편집기_최소_사이로_고정된다() {
    let cur = w(380, 360);
    assert_eq!(drag_problem(1920, THREE, cur, 500), w(500, 360));
    assert_eq!(
        drag_problem(1920, THREE, cur, 50),
        w(MIN_PROBLEM_WIDTH, 360)
    );
    assert_eq!(
        drag_problem(1920, THREE, cur, -40),
        w(MIN_PROBLEM_WIDTH, 360)
    );
    assert_eq!(
        drag_problem(1920, THREE, cur, 9999),
        w(1920 - MIN_WORK_WIDTH - 360, 360)
    );
}

#[test]
fn ai_경계를_끌면_최소와_편집기_최소_사이로_고정된다() {
    let cur = w(380, 360);
    assert_eq!(drag_ai(1920, THREE, cur, 450), w(380, 450));
    assert_eq!(drag_ai(1920, THREE, cur, 10), w(380, MIN_AI_WIDTH));
    assert_eq!(
        drag_ai(1920, THREE, cur, 9999),
        w(380, 1920 - MIN_WORK_WIDTH - 380)
    );
}

#[test]
fn 접힘_모드는_ai_경계만_끌린다() {
    let cur = w(FOLDED_RAIL_WIDTH, 320);
    assert_eq!(drag_problem(1300, FOLDED, cur, 600), cur);
    assert_eq!(drag_ai(1300, FOLDED, cur, 420), w(FOLDED_RAIL_WIDTH, 420));
}

#[test]
fn 설정_기본값과_배치별_폭() {
    let p = LayoutPrefs::default();
    assert_eq!(p.widths(THREE), w(DEFAULT_PROBLEM_WIDTH, DEFAULT_AI_WIDTH));
    assert_eq!(
        p.widths(FOLDED),
        w(FOLDED_RAIL_WIDTH, DEFAULT_FOLDED_AI_WIDTH)
    );
}

#[test]
fn 넓은_모드와_접힘_모드_폭은_따로_기억한다() {
    let mut p = LayoutPrefs::default();
    p.set(THREE, w(300, 400));
    p.set(FOLDED, w(FOLDED_RAIL_WIDTH, 350));
    assert_eq!(p.widths(THREE), w(300, 400));
    assert_eq!(p.widths(FOLDED).ai, 350);
}

#[test]
fn 설정은_쓰고_다시_읽으면_같다() {
    let dir = std::env::temp_dir().join(format!("pl-layout-{}", std::process::id()));
    let path = dir.join("nested").join(LAYOUT_FILE);
    let mut p = LayoutPrefs::default();
    p.set(THREE, w(333, 444));
    p.save(&path).unwrap();
    assert_eq!(LayoutPrefs::load(&path), p);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn 없거나_깨졌거나_모르는_판이면_기본값() {
    let dir = std::env::temp_dir().join(format!("pl-layout-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(LAYOUT_FILE);
    assert_eq!(LayoutPrefs::load(&path), LayoutPrefs::default());
    std::fs::write(&path, "{ 깨짐").unwrap();
    assert_eq!(LayoutPrefs::load(&path), LayoutPrefs::default());
    std::fs::write(
        &path,
        r#"{"version":999,"three":{"problem":1,"ai":2},"folded_ai":3}"#,
    )
    .unwrap();
    assert_eq!(LayoutPrefs::load(&path), LayoutPrefs::default());
    std::fs::write(&path, r#"{"version":1}"#).unwrap();
    assert_eq!(LayoutPrefs::load(&path), LayoutPrefs::default());
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn 쓰기_실패는_오류로_돌려준다() {
    let dir = std::env::temp_dir().join(format!("pl-layout-ro-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // 폴더 자리에 파일이 있으면 쓸 수 없다.
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    assert!(
        LayoutPrefs::default()
            .save(&blocker.join(LAYOUT_FILE))
            .is_err()
    );
    std::fs::remove_file(&blocker).unwrap();
}
