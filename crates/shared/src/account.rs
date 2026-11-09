//! 조직(테넌트) 역할과 권한. 서버가 API에서 이 표로 권한을 강제하고, 대시보드는 같은 표로 버튼을 숨긴다.
//!
//! | 역할 | 구성원 관리 | 시험·문제 관리 | 감독 | 응시 결과 보기 |
//! |---|---|---|---|---|
//! | 관리자 `admin` | O | O | O | O |
//! | 출제자 `author` | | O | | |
//! | 감독관 `proctor` | | | O | O |
//! | 열람자 `viewer` | | | | O |
//!
//! 출제자는 문제·시험을 만들 뿐 응시자 결과(개인정보)는 보지 않는다. 구성원이 아닌 사람에게는
//! 조직이 있는지조차 알리지 않는다(서버는 404로 답한다).

use serde::{Deserialize, Serialize};

/// 조직 안에서의 역할.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Role {
    Admin,
    Author,
    Proctor,
    Viewer,
}

/// 역할이 여는 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// 동료 초대, 역할 바꾸기, 구성원 내보내기.
    ManageMembers,
    /// 문제·시험 만들기·고치기(#8~).
    ManageExams,
    /// 실시간 감독(#12~).
    Proctor,
    /// 조직의 응시 결과 보기.
    ViewResults,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Admin, Role::Author, Role::Proctor, Role::Viewer];

    /// DB·API에 쓰는 이름.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Author => "author",
            Role::Proctor => "proctor",
            Role::Viewer => "viewer",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// 화면에 보이는 이름.
    pub fn label(self) -> &'static str {
        match self {
            Role::Admin => "관리자",
            Role::Author => "출제자",
            Role::Proctor => "감독관",
            Role::Viewer => "열람자",
        }
    }

    pub fn allows(self, permission: Permission) -> bool {
        use Permission::*;
        match self {
            Role::Admin => true,
            Role::Author => matches!(permission, ManageExams),
            Role::Proctor => matches!(permission, Proctor | ViewResults),
            Role::Viewer => matches!(permission, ViewResults),
        }
    }
}
