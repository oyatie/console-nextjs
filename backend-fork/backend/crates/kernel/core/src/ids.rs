//! Typed UUID newtypes. A `WorkOrderId` can never be passed where a `UserId`
//! is expected; the type system carries the discipline the prior project kept
//! in naming conventions.

/// Defines a UUID-backed ID newtype with serde, parsing, and display.
macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// Generates a fresh random ID.
            #[must_use]
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: uuid::Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(uuid::Uuid::parse_str(s)?))
            }
        }

        impl From<$name> for uuid::Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

typed_id!(
    /// A tenant — one maintenance company (정비 회사). The top of the
    /// hierarchy (org → region → branch → user) and the hard multi-tenant
    /// isolation boundary: every tenant-scoped row carries an `org_id`, and
    /// Postgres RLS keyed on `app.current_org` makes cross-tenant access
    /// impossible. KNL Logistics is tenant #1.
    OrgId
);

impl OrgId {
    /// Tenant #1 — KNL Logistics. The deployment is single-tenant until the
    /// per-request tenant resolver lands, so every server-side write path arms
    /// `app.current_org` with this fixed id. It MUST byte-for-byte match the id
    /// seeded by migration `0028_backfill_org_id.sql`.
    #[must_use]
    pub const fn knl() -> Self {
        Self(uuid::Uuid::from_u128(0xa1))
    }

    /// The PLATFORM sentinel — the SaaS-vendor tier that sits ABOVE every
    /// tenant. It is deliberately NOT a real tenant: no `organizations` row has
    /// this id, no tenant data carries it as `org_id`, and arming it as
    /// `app.current_org` selects ZERO tenant rows (every RLS policy compares the
    /// GUC to a real per-tenant `org_id`, which can never equal this marker).
    ///
    /// A platform token stamps this into its `org` claim purely so the claim is
    /// a valid UUID; the platform tier is authorized by the `platform = true`
    /// claim, never by this value. Distinct from `knl()` (`0xa1`) and from the
    /// nil UUID so it can never be confused with an unset/zeroed id. The marker
    /// is `0x...00face` (a recognizable non-tenant byte pattern).
    #[must_use]
    pub const fn platform() -> Self {
        Self(uuid::Uuid::from_u128(0x00face_u128))
    }
}
typed_id!(
    /// An organizational branch (지점). Day-1 scoping concept: every
    /// work order, user membership, equipment row, KPI rollup, and chat
    /// team-channel carries one.
    BranchId
);
typed_id!(
    /// A region (지역) grouping branches (수도권/충청/영남/호남 rollout units).
    RegionId
);
typed_id!(UserId);
typed_id!(
    /// A non-human credential holder used by an external production source.
    /// This is deliberately distinct from [`UserId`]: a service principal can
    /// never be issued a human session, assigned a role, or treated as a
    /// member of the organization.
    ServicePrincipalId
);
typed_id!(EquipmentId);
typed_id!(CustomerId);
typed_id!(SiteId);
typed_id!(WorkOrderId);
typed_id!(AssignmentId);
typed_id!(EquipmentSubstitutionId);
typed_id!(ApprovalId);
typed_id!(DailyPlanId);
typed_id!(ConsentId);
typed_id!(LocationPingId);
typed_id!(P1DispatchId);
typed_id!(P1DispatchResponseId);
typed_id!(P1DispatchAlertId);
typed_id!(InventoryItemId);
typed_id!(InventoryStockLocationId);
typed_id!(InventoryConsumptionEventId);
typed_id!(BenefitCatalogItemId);
typed_id!(BenefitCatalogTierId);
typed_id!(BenefitCatalogConditionId);
typed_id!(ThreadId);
typed_id!(MessageId);
typed_id!(EvidenceId);
typed_id!(EvidenceObjectId);
typed_id!(EvidenceCopyId);
typed_id!(EvidenceTsaProofId);
typed_id!(EvidenceCustodyEventId);
typed_id!(EvidenceLegalHoldId);
typed_id!(EvidenceExportId);
typed_id!(DeviceId);
typed_id!(VendorId);
typed_id!(QuoteId);
typed_id!(PurchaseRequestId);
typed_id!(InspectionId);
typed_id!(InspectionScheduleId);
typed_id!(InspectionRoundId);
typed_id!(SalesListingId);
typed_id!(CustomerInquiryId);
typed_id!(SupportTicketId);
typed_id!(SupportTicketCommentId);
typed_id!(AuditEventId);
typed_id!(
    /// A notification — one recipient-scoped pointer object in the general
    /// notifications domain (결재/멘션/문서/공지/근태/급여, extensible). Persistence
    /// is the source of truth; realtime `LISTEN/NOTIFY` carries only this id.
    NotificationId
);
typed_id!(
    /// A personal todo — one owner-scoped action item in the todos domain
    /// (UI-M3 Overview Today/Plan panel). Owner-scoped like notifications:
    /// the owner is always bound from the authenticated principal.
    TodoId
);
typed_id!(
    /// One document in a recipient's statutory-notice vault (개인 수신함): a
    /// payslip self-view row (frictionless) or a legal notice (근로계약/취업규칙/
    /// 연차촉진/노무수령거부) whose passkey-gated confirmation is the legal receipt
    /// evidence. Recipient-scoped like notifications; the recipient is always
    /// bound from the authenticated principal, never from request input.
    InboxDocId
);
typed_id!(
    /// One leave request (연차/반차 신청) awaiting a branch approver's decision.
    /// Branch-scoped; the requesting principal is bound from the authenticated
    /// token, and the decider is separated from the requester (SoD).
    LeaveRequestId
);
typed_id!(
    /// One statutory leave push (근로기준법 §61 연차 사용 촉진 1차/2차, or a
    /// 노무수령거부 notice) delivered to a target employee's 개인 수신함. Records the
    /// receipt-document delivery and, when the submittable definition exists, the
    /// engine AP- submission it started.
    LeavePromotionId
);
typed_id!(
    /// One org-wide board notice (게시판 NT- 공지): a draft -> published
    /// document. Publishing snapshots recipients into [`NoticeReceiptId`] rows
    /// and fans out one `notifications`-table pointer per recipient.
    NoticeId
);
typed_id!(
    /// One recipient's 수령확인 (receipt acknowledgment) row for a
    /// [`NoticeId`], snapshotted at publish time.
    NoticeReceiptId
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn ids_are_distinct_types_and_roundtrip_serde() {
        let id = WorkOrderId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: WorkOrderId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
        // transparent serde: plain UUID string, no wrapper object
        assert!(json.starts_with('"') && json.ends_with('"'));
    }

    #[test]
    fn ids_parse_from_canonical_uuid_strings() {
        let id = BranchId::new();
        let parsed = BranchId::from_str(&id.to_string()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn invalid_uuid_string_is_rejected() {
        assert!(UserId::from_str("not-a-uuid").is_err());
    }

    #[test]
    fn fresh_ids_are_unique() {
        assert_ne!(UserId::new(), UserId::new());
    }
}
