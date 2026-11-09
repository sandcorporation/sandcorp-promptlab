//! 역할·권한 표(`account.rs` 문서의 표와 같아야 한다). 서버가 이 표로 API 권한을 강제한다.

use promptlab_shared::account::{Permission, Role};

#[test]
fn 역할별_권한_표() {
    use Permission::*;
    let table = [
        (Role::Admin, [true, true, true, true]),
        (Role::Author, [false, true, false, false]),
        (Role::Proctor, [false, false, true, true]),
        (Role::Viewer, [false, false, false, true]),
    ];
    for (role, expected) in table {
        let actual = [ManageMembers, ManageExams, Proctor, ViewResults].map(|p| role.allows(p));
        assert_eq!(actual, expected, "{role:?}");
    }
}

#[test]
fn 역할_이름은_왕복한다() {
    for role in Role::ALL {
        assert_eq!(Role::parse(role.as_str()), Some(role));
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, format!("\"{}\"", role.as_str()));
    }
    assert_eq!(Role::parse("owner"), None);
}
