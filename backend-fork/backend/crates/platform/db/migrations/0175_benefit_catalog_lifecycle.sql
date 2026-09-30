-- Benefit catalog items are governed objects. New rows are registered by the
-- catalog transaction; this migration backfills existing rows and declares the
-- same explicit FSM used by the Console.
INSERT INTO lifecycle_transition_rules (object_type, from_state, to_state) VALUES
    ('benefit_catalog_item', 'draft', 'pending'),
    ('benefit_catalog_item', 'pending', 'finalized'),
    ('benefit_catalog_item', 'finalized', 'implemented'),
    ('benefit_catalog_item', 'implemented', 'retiring'),
    ('benefit_catalog_item', 'retiring', 'retired')
ON CONFLICT DO NOTHING;

INSERT INTO object_lifecycles (org_id, object_type, object_id, current_state)
SELECT org_id, 'benefit_catalog_item', id, 'draft'
FROM benefit_catalog_items
ON CONFLICT (org_id, object_type, object_id) DO NOTHING;
