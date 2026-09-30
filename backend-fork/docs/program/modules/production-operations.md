# Production operations pilot

`CAP-PRODUCTION-PILOT` is an additive, dark Console vertical. Its first real story is a planner's production plan: a customer-demand reference plus explicit capacity, material, and staffing check references creates one draft plan and first operation; an authorized reviewer releases it; an authorized operator records output, scrap, downtime, and quality evidence.

The production store is intentionally narrow. It persists only plan/operation state and immutable lifecycle events. Customer demand, people/staffing, inventory/material, approval, ontology, and reporting are ports represented by stable references and check snapshots; this vertical does not copy or join those source stores.

Every record is tenant-scoped with forced PostgreSQL RLS. Mutations require an idempotency key and use optimistic versions. Release and execution records reject stale versions or invalid lifecycle transitions. `production_plan_events` provides durable plan lineage with actor, timestamp, payload, and idempotency key. The API remains dark pending independent runtime/evidence approval; the web surface uses the API contract and does not synthesize production data.
