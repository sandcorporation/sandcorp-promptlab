-- 이용약관 동의(#86): 조직을 만들 때·구성원 초대를 받을 때 동의한 약관 판을 남긴다.
-- 이전에 만든 조직·구성원에는 기록이 없다(출시 전이라 재동의·이전을 짓지 않는다).
CREATE TABLE terms_acceptances (
    org_id      UUID        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    sub         TEXT        NOT NULL,
    version     TEXT        NOT NULL,
    via         TEXT        NOT NULL CHECK (via IN ('create_org', 'invite')),
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, sub)
);
