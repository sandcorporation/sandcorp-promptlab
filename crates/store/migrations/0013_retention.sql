-- 데이터 보유·자동 삭제(#54). 규칙은 promptlab_shared::retention.
-- 보관 기간은 시험별 → 조직 기본 → 365일. 열을 NULL로 더하므로 기존 조직·시험은 모두 기본 365일이 된다
-- (운영 DB를 손으로 고치지 않는다). 허용 범위(30~1095일)는 서버가 강제하고, 여기서는 양수만 건다.
ALTER TABLE orgs ADD COLUMN retention_days integer CHECK (retention_days > 0);
ALTER TABLE exams
    ADD COLUMN retention_days integer CHECK (retention_days > 0),
    -- 보관 기간이 지나 지운 배정 중 응시자가 있던 것의 수. 사용량 화면의 응시 수가 삭제 뒤에도 줄지 않게 익명 숫자로만 남긴다.
    ADD COLUMN erased_attempts integer NOT NULL DEFAULT 0 CHECK (erased_attempts >= 0);

-- 보관 기간 변경 이력(덧붙이기만). exam_id 가 NULL이면 조직 기본값을 바꾼 것.
CREATE TABLE retention_changes (
    id         bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    org_id     uuid        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    exam_id    uuid        REFERENCES exams (id) ON DELETE CASCADE,
    changed_by text        NOT NULL REFERENCES users (sub),
    changed_at timestamptz NOT NULL,
    old_days   integer,
    new_days   integer
);
CREATE INDEX retention_changes_org_idx ON retention_changes (org_id, changed_at);

-- 배정 종료 시각을 잴 때 그 배정의 제출을 찾는다(유일 색인 submissions_invite_item_idx 가 이미 invite_id 앞머리다).
-- 개인 연습 무활동은 submissions_practice_idx(candidate_sub, created_at) WHERE practice 를 쓴다.
