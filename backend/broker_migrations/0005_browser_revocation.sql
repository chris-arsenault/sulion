CREATE TABLE secret_broker.browser_revocations (
    user_sub TEXT PRIMARY KEY,
    revoked_before BIGINT NOT NULL
);
