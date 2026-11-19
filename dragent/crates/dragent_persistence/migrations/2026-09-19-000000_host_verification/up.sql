CREATE TABLE host_command_verifications (
    id TEXT PRIMARY KEY NOT NULL,
    work_item_id TEXT NOT NULL,
    candidate_attempt_id TEXT NOT NULL,
    receipt_json TEXT NOT NULL
);
CREATE INDEX host_verification_candidate ON host_command_verifications(work_item_id, candidate_attempt_id);
