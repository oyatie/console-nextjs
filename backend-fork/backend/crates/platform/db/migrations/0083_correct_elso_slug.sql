-- Correct the legacy Elso tenant slug. The canonical slug is `lso`, not `elso`.
-- Guard against environments where `lso` was already corrected manually.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM organizations
        WHERE slug = 'elso'
          AND name ILIKE '%엘소%'
    ) AND NOT EXISTS (
        SELECT 1
        FROM organizations
        WHERE slug = 'lso'
    ) THEN
        UPDATE organizations
        SET slug = 'lso',
            updated_at = now()
        WHERE slug = 'elso'
          AND name ILIKE '%엘소%';
    END IF;
END $$;
