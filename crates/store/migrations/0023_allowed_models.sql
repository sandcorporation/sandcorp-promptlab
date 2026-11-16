-- 허용 AI 모델(#5): 시험이 허용하는 후보 모델 id(1개 이상, 기본 후보 전부 — promptlab_shared::exam::all_models).
-- 허용 밖 모델로 문항을 시작하면 서버가 403으로 막는다(시작 시각을 남기지 않는다).
ALTER TABLE exams ADD COLUMN allowed_models jsonb NOT NULL
    DEFAULT '["7b","1.5b"]'::jsonb
    CHECK (jsonb_typeof(allowed_models) = 'array' AND jsonb_array_length(allowed_models) >= 1);

-- 허용 모델 거부를 무결성 거부(허용 목록)와 따로 센다: 같은 표에 종류를 둔다(삭제·보관 경로가 같다).
ALTER TABLE exam_integrity_rejections ADD COLUMN kind text NOT NULL DEFAULT 'allowlist'
    CHECK (kind IN ('allowlist', 'model_not_allowed'));
ALTER TABLE exam_integrity_rejections DROP CONSTRAINT exam_integrity_rejections_pkey;
ALTER TABLE exam_integrity_rejections ADD PRIMARY KEY (invite_id, position, kind, app_version, model_sha256);

-- AI 속도·GPU(#5): 제출 때 과정 로그의 마지막 Submitted 사건(서명됨)에 앱이 신고한 생성 속도·GPU 층 수
-- (promptlab_shared::log::AiSpeed). 신고값이라 model_sha256과 같은 성격이다(고친 앱은 속일 수 있다). 없으면 NULL.
ALTER TABLE exam_item_starts ADD COLUMN ai_speed jsonb;
