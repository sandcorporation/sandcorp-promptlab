-- 개인 연습(#18). 조직·시험 배정 없이 낸 제출은 개인 연습이다 — 낸 사람만 본다.
-- 운영자 집계(#19)와 고객사 집계가 구분할 수 있게 열로 표시한다(고객사 집계는 시험 배정에서 출발하므로 원래 안 나온다).
ALTER TABLE submissions ADD COLUMN practice boolean NOT NULL DEFAULT false;
-- 지금까지의 개인 제출(조직·배정 없음)도 연습이다.
UPDATE submissions SET practice = true WHERE org_id IS NULL AND invite_id IS NULL;
ALTER TABLE submissions
    ADD CONSTRAINT submissions_practice_personal CHECK (NOT practice OR (org_id IS NULL AND invite_id IS NULL));

-- 채점 큐: 연습은 시험(과 조직 제출) 뒤에 꺼낸다(practice=false가 먼저).
CREATE INDEX submissions_queue_idx ON submissions (practice, created_at) WHERE status = 'queued';
DROP INDEX submissions_queued_idx;

-- 내 연습 기록(사람마다 시간순).
CREATE INDEX submissions_practice_idx ON submissions (candidate_sub, created_at) WHERE practice;
