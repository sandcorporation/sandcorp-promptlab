//! 보관 기간 규칙(#54).

use promptlab_shared::retention::{
    DEFAULT_DAYS, Erased, MAX_DAYS, MIN_DAYS, days_left, delete_at, effective_days, validate_days,
};

#[test]
fn 시험별_값이_조직_기본보다_먼저고_둘_다_없으면_1년() {
    assert_eq!(DEFAULT_DAYS, 365);
    assert_eq!(effective_days(None, None), 365);
    assert_eq!(effective_days(None, Some(90)), 90);
    assert_eq!(effective_days(Some(60), Some(90)), 60);
}

#[test]
fn 허용_범위_밖은_거부한다() {
    assert!(validate_days(MIN_DAYS).is_ok());
    assert!(validate_days(MAX_DAYS).is_ok());
    assert!(validate_days(MIN_DAYS - 1).is_err());
    assert!(validate_days(MAX_DAYS + 1).is_err());
    assert!(validate_days(0).is_err());
}

#[test]
fn 삭제_시각과_남은_날() {
    let end = 1_000_000;
    let at = delete_at(end, 365);
    assert_eq!(at, end + 365 * 86_400);
    assert_eq!(days_left(at, end), 365);
    assert_eq!(days_left(at, at - 1), 1);
    assert_eq!(days_left(at, at), 0);
    assert_eq!(days_left(at, at + 10), 0);
}

#[test]
fn 지운_건수_더하기() {
    let mut a = Erased {
        assignments: 1,
        submissions: 2,
        ..Erased::default()
    };
    assert!(!a.is_empty());
    a.add(&Erased {
        submissions: 3,
        usage_unlinked: 4,
        ..Erased::default()
    });
    assert_eq!(a.submissions, 5);
    assert_eq!(a.usage_unlinked, 4);
    assert!(Erased::default().is_empty());
}
