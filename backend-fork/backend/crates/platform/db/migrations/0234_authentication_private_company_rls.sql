-- Expand-only Company isolation for the dormant authentication command role.
-- The private records already have Company columns; restrict direct command
-- access before any credential or caller is attached to console_auth_cmd.
-- The table owner retains trigger, migration and restore access under FORCE RLS.

ALTER TABLE auth_security.account_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth_security.account_state FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_command_company ON auth_security.account_state
    TO console_auth_cmd
    USING (home_org_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID)
    WITH CHECK (home_org_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID);
CREATE POLICY auth_owner_all ON auth_security.account_state
    TO console_app USING (true) WITH CHECK (true);

ALTER TABLE auth_security.credential_removals ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth_security.credential_removals FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_command_company ON auth_security.credential_removals
    TO console_auth_cmd
    USING (company_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID)
    WITH CHECK (company_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID);
CREATE POLICY auth_owner_all ON auth_security.credential_removals
    TO console_app USING (true) WITH CHECK (true);

ALTER TABLE auth_security.session_admissions ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth_security.session_admissions FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_command_company ON auth_security.session_admissions
    TO console_auth_cmd
    USING (home_org_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID)
    WITH CHECK (home_org_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID);
CREATE POLICY auth_owner_all ON auth_security.session_admissions
    TO console_app USING (true) WITH CHECK (true);

ALTER TABLE auth_security.registration_intents ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth_security.registration_intents FORCE ROW LEVEL SECURITY;
CREATE POLICY auth_command_company ON auth_security.registration_intents
    TO console_auth_cmd
    USING (home_org_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID)
    WITH CHECK (home_org_id = NULLIF(pg_catalog.current_setting('app.current_org', true), '')::UUID);
CREATE POLICY auth_owner_all ON auth_security.registration_intents
    TO console_app USING (true) WITH CHECK (true);

-- The Company discriminator must also agree with the Account identity; RLS
-- alone would accept a row that names another Company's Account while carrying
-- the currently armed Company. Security-state rows outlive Account deletion.
ALTER TABLE auth_security.account_state
    ADD CONSTRAINT account_state_account_home_unique UNIQUE (account_id, home_org_id);
ALTER TABLE auth_security.session_admissions
    ADD CONSTRAINT session_admission_account_home_fk
    FOREIGN KEY (account_id, home_org_id)
    REFERENCES auth_security.account_state (account_id, home_org_id)
    ON UPDATE RESTRICT ON DELETE RESTRICT;
ALTER TABLE auth_security.registration_intents
    ADD CONSTRAINT registration_intent_account_home_fk
    FOREIGN KEY (account_id, home_org_id)
    REFERENCES auth_security.account_state (account_id, home_org_id)
    ON UPDATE RESTRICT ON DELETE RESTRICT;
