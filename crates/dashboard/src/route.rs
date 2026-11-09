//! 대시보드 페이지 주소(#75). 주소는 해시(`#/orgs/{id}/exams` …)에 둔다 — 서버가 같은 API를 `/`와 `/api` 두 곳에서
//! 받으므로 `/orgs/…` 같은 경로는 API와 겹친다. 해시는 서버로 가지 않고, 뒤로 가기·새로고침·링크 복사가 된다.
//! 화면은 [`Route`] 하나로 정해지고 링크는 진짜 `<a href>`(키보드·새 탭)다. 쓰는 곳은 [`crate::shell`].

/// 시험 상세 안의 탭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExamTab {
    /// 응시자(초대 현황 — 과정 다시보기·의심 신호·웹캠·리포트·삭제).
    Candidates,
    /// 응시 결과(점수·CSV·공개 설정).
    Results,
    /// 문항과 채점(문항·보관 기간).
    Items,
    /// 회차 비교(조직의 사내 진단).
    Rounds,
    /// 실시간 감독(응시 가능 시간·실시간 보기).
    Live,
}

impl ExamTab {
    pub const ALL: [ExamTab; 5] = [
        ExamTab::Candidates,
        ExamTab::Results,
        ExamTab::Items,
        ExamTab::Rounds,
        ExamTab::Live,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            ExamTab::Candidates => "candidates",
            ExamTab::Results => "results",
            ExamTab::Items => "items",
            ExamTab::Rounds => "rounds",
            ExamTab::Live => "live",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExamTab::Candidates => "응시자",
            ExamTab::Results => "응시 결과",
            ExamTab::Items => "문항과 채점",
            ExamTab::Rounds => "회차 비교",
            ExamTab::Live => "실시간 감독",
        }
    }

    fn parse(s: &str) -> Option<ExamTab> {
        ExamTab::ALL.into_iter().find(|t| t.slug() == s)
    }
}

/// 운영자 화면의 탭(#19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorTab {
    Orgs,
    Problems,
    Metrics,
    Allowlist,
}

impl OperatorTab {
    pub const ALL: [OperatorTab; 4] = [
        OperatorTab::Orgs,
        OperatorTab::Problems,
        OperatorTab::Metrics,
        OperatorTab::Allowlist,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            OperatorTab::Orgs => "orgs",
            OperatorTab::Problems => "problems",
            OperatorTab::Metrics => "metrics",
            OperatorTab::Allowlist => "allowlist",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OperatorTab::Orgs => "시범 조직",
            OperatorTab::Problems => "문제은행",
            OperatorTab::Metrics => "큐·비용",
            OperatorTab::Allowlist => "허용 목록",
        }
    }

    fn parse(s: &str) -> Option<OperatorTab> {
        OperatorTab::ALL.into_iter().find(|t| t.slug() == s)
    }
}

/// 응시자 상세 안의 탭(#75 C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandTab {
    /// 점수(제출 코드·편집 기록은 #100).
    Conversations,
    Replay,
    Signals,
    Webcam,
    Report,
}

impl CandTab {
    pub const ALL: [CandTab; 5] = [
        CandTab::Conversations,
        CandTab::Replay,
        CandTab::Signals,
        CandTab::Webcam,
        CandTab::Report,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            CandTab::Conversations => "conversations",
            CandTab::Replay => "replay",
            CandTab::Signals => "signals",
            CandTab::Webcam => "webcam",
            CandTab::Report => "report",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CandTab::Conversations => "점수",
            CandTab::Replay => "과정 다시보기",
            CandTab::Signals => "의심 신호",
            CandTab::Webcam => "웹캠 사진",
            CandTab::Report => "AI 리포트",
        }
    }

    fn parse(s: &str) -> Option<CandTab> {
        CandTab::ALL.into_iter().find(|t| t.slug() == s)
    }
}

/// 조직 안의 페이지(왼쪽 메뉴 한 칸 또는 그 아래).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgPage {
    /// 시험 목록.
    Exams,
    /// 새 시험.
    NewExam,
    /// 시험 상세.
    Exam { exam: String, tab: ExamTab },
    /// 응시자 상세(응시 결과에서 들어간다).
    Candidate {
        exam: String,
        invite: String,
        tab: CandTab,
    },
    /// 우리 조직 문제.
    Problems,
    /// 실시간 감독(시험 고르기).
    Live,
    /// 사용량·추정 비용.
    Usage,
    /// 구성원·동료 초대.
    Members,
    /// 보관 기간·응시자 데이터 삭제 기록.
    Retention,
}

/// 대시보드 페이지 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// 내 조직(조직 고르기·만들기).
    Home,
    Org {
        org: String,
        page: OrgPage,
    },
    /// 내 연습 기록(#18).
    Practice,
    /// 운영자 화면(대표만).
    Operator(OperatorTab),
    /// 구성원 초대 수락(약관 동의 뒤 들어간다, #86).
    Invite(String),
}

impl Route {
    /// 해시(`#` 뒤, 앞의 `#`는 있어도 없어도 된다)를 읽는다. 모르는 주소는 [`Route::Home`].
    pub fn parse(hash: &str) -> Route {
        let path = hash.trim_start_matches('#').trim_matches('/');
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        match parts.as_slice() {
            ["practice"] => Route::Practice,
            ["invite", token] => Route::Invite((*token).to_owned()),
            ["operator"] => Route::Operator(OperatorTab::Orgs),
            ["operator", tab] => {
                Route::Operator(OperatorTab::parse(tab).unwrap_or(OperatorTab::Orgs))
            }
            ["orgs", org, rest @ ..] => {
                let page = match rest {
                    [] | ["exams"] => Some(OrgPage::Exams),
                    ["exams", "new"] => Some(OrgPage::NewExam),
                    ["exams", exam] => Some(OrgPage::Exam {
                        exam: (*exam).to_owned(),
                        tab: ExamTab::Candidates,
                    }),
                    ["exams", exam, tab] => ExamTab::parse(tab).map(|tab| OrgPage::Exam {
                        exam: (*exam).to_owned(),
                        tab,
                    }),
                    ["exams", exam, "candidates", invite] => Some(OrgPage::Candidate {
                        exam: (*exam).to_owned(),
                        invite: (*invite).to_owned(),
                        tab: CandTab::Conversations,
                    }),
                    ["exams", exam, "candidates", invite, tab] => {
                        CandTab::parse(tab).map(|tab| OrgPage::Candidate {
                            exam: (*exam).to_owned(),
                            invite: (*invite).to_owned(),
                            tab,
                        })
                    }
                    ["problems"] => Some(OrgPage::Problems),
                    ["live"] => Some(OrgPage::Live),
                    ["usage"] => Some(OrgPage::Usage),
                    ["members"] => Some(OrgPage::Members),
                    ["retention"] => Some(OrgPage::Retention),
                    _ => None,
                };
                match page {
                    Some(page) => Route::Org {
                        org: (*org).to_owned(),
                        page,
                    },
                    None => Route::Home,
                }
            }
            _ => Route::Home,
        }
    }

    /// 링크(`href`)로 쓸 해시 주소.
    pub fn href(&self) -> String {
        match self {
            Route::Home => "#/".into(),
            Route::Practice => "#/practice".into(),
            Route::Invite(token) => format!("#/invite/{token}"),
            Route::Operator(tab) => format!("#/operator/{}", tab.slug()),
            Route::Org { org, page } => {
                let rest = match page {
                    OrgPage::Exams => "exams".to_owned(),
                    OrgPage::NewExam => "exams/new".to_owned(),
                    OrgPage::Exam { exam, tab } => format!("exams/{exam}/{}", tab.slug()),
                    OrgPage::Candidate { exam, invite, tab } => {
                        format!("exams/{exam}/candidates/{invite}/{}", tab.slug())
                    }
                    OrgPage::Problems => "problems".to_owned(),
                    OrgPage::Live => "live".to_owned(),
                    OrgPage::Usage => "usage".to_owned(),
                    OrgPage::Members => "members".to_owned(),
                    OrgPage::Retention => "retention".to_owned(),
                };
                format!("#/orgs/{org}/{rest}")
            }
        }
    }

    /// 조직 페이지 주소.
    pub fn org(org: &str, page: OrgPage) -> Route {
        Route::Org {
            org: org.to_owned(),
            page,
        }
    }

    /// 이 페이지가 속한 조직.
    pub fn org_id(&self) -> Option<&str> {
        match self {
            Route::Org { org, .. } => Some(org),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 주소는_읽고_쓰면_같다() {
        let routes = [
            Route::Home,
            Route::Practice,
            Route::Invite("tok".into()),
            Route::Operator(OperatorTab::Metrics),
            Route::org("o1", OrgPage::Exams),
            Route::org("o1", OrgPage::NewExam),
            Route::org(
                "o1",
                OrgPage::Exam {
                    exam: "e1".into(),
                    tab: ExamTab::Live,
                },
            ),
            Route::org(
                "o1",
                OrgPage::Candidate {
                    exam: "e1".into(),
                    invite: "i1".into(),
                    tab: CandTab::Report,
                },
            ),
            Route::org("o1", OrgPage::Problems),
            Route::org("o1", OrgPage::Live),
            Route::org("o1", OrgPage::Usage),
            Route::org("o1", OrgPage::Members),
            Route::org("o1", OrgPage::Retention),
        ];
        for route in routes {
            assert_eq!(Route::parse(&route.href()), route, "{}", route.href());
        }
    }

    #[test]
    fn 모르는_주소는_홈이고_빈_탭은_응시자다() {
        assert_eq!(Route::parse(""), Route::Home);
        assert_eq!(Route::parse("#/nope"), Route::Home);
        assert_eq!(Route::parse("#/orgs/o1/nope"), Route::Home);
        assert_eq!(Route::parse("#/orgs/o1"), Route::org("o1", OrgPage::Exams));
        assert_eq!(
            Route::parse("#/orgs/o1/exams/e1"),
            Route::org(
                "o1",
                OrgPage::Exam {
                    exam: "e1".into(),
                    tab: ExamTab::Candidates
                }
            )
        );
        assert_eq!(
            Route::parse("#/operator/zzz"),
            Route::Operator(OperatorTab::Orgs)
        );
    }
}
