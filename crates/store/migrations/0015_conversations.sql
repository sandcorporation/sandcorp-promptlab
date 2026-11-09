-- 대화형 채점(#71, spec #69): 제출 = 대화. 옛 「제출 프롬프트」 행은 지우지 않고 읽기 전용으로 남긴다.

-- 제출: 옛 방식은 prompt, 새 방식은 대화들(conversations) + 고른 대화(conversation_id). 둘 중 하나는 있어야 한다.
ALTER TABLE submissions ALTER COLUMN prompt DROP NOT NULL;
ALTER TABLE submissions ADD COLUMN conversation_id text;
-- 이 문항에서 연 대화 전부(shared::conversation::Conversation 배열). 시험 응시는 제출 때 과정 로그에서 다시 세운 것.
ALTER TABLE submissions ADD COLUMN conversations jsonb;
ALTER TABLE submissions ADD COLUMN calls integer CHECK (calls >= 0);
ALTER TABLE submissions
    ADD CONSTRAINT submissions_prompt_or_conversation
    CHECK (prompt IS NOT NULL OR (conversations IS NOT NULL AND conversation_id IS NOT NULL));
-- 채점을 기다리던 옛 방식 제출은 새 워커가 채점하지 않는다(옛 단일 턴 채점 경로가 없다).
UPDATE submissions
   SET status = 'failed', failure_reason = '옛 방식(제출 프롬프트) 제출이라 채점하지 않았다',
       failure_detail = '#71 대화형 전환 때 큐에 남아 있었다', finished_at = now()
 WHERE status IN ('queued', 'grading') AND conversations IS NULL;

-- 시험 설정: 최종 점수 비율 w(%)·대화 길이 상한(토큰)·문항당 모델 호출 수 상한·추가 기준표(최대 2).
ALTER TABLE exams ADD COLUMN score_weight smallint NOT NULL DEFAULT 50 CHECK (score_weight BETWEEN 0 AND 100);
ALTER TABLE exams ADD COLUMN max_tokens integer NOT NULL DEFAULT 3000 CHECK (max_tokens > 0);
ALTER TABLE exams ADD COLUMN max_calls integer NOT NULL DEFAULT 30 CHECK (max_calls > 0);
ALTER TABLE exams ADD COLUMN rubric jsonb NOT NULL DEFAULT '[]'::jsonb;
-- 문항별 최대 턴은 시험의 호출 상한으로 바뀌었다. 옛 값은 남기되(읽지 않는다) 새 문항은 넣지 않는다.
ALTER TABLE exam_items ALTER COLUMN max_turns DROP NOT NULL;

-- 대조 큐(#12)를 제출 대화의 모델 답으로 넓힌다: 대조할 턴의 위치(대화 안 0부터). 옛 공개 예시 대조 행은 NULL.
ALTER TABLE exam_example_rechecks ADD COLUMN turn integer CHECK (turn >= 0);
-- 기다리던 옛 공개 예시 대조는 새 워커가 돌리지 않는다.
UPDATE exam_example_rechecks
   SET status = 'failed', failure = '옛 방식(공개 예시 대조) — 대화형 전환으로 건너뜀', checked_at = now()
 WHERE status IN ('pending', 'running') AND turn IS NULL;

-- 기준 프롬프트 풀림 미리 돌리기는 꺼졌다(#73이 모범 대화·기준선 검사로 다시 쓴다). 기다리던 것은 끝낸다.
UPDATE problem_previews
   SET status = 'failed', failure_reason = '기준 프롬프트 미리 돌리기는 대화형 전환으로 꺼졌다', finished_at = now()
 WHERE status IN ('queued', 'running');
