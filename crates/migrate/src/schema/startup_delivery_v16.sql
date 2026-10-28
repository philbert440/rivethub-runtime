ALTER TABLE rh_session_startup
    ADD COLUMN delivered_via TEXT NOT NULL DEFAULT 'none'
    CHECK (delivered_via IN ('metaAppend', 'cliFlag', 'none'));
