//! Integration-level qualification of the production authority vocabulary.
//!
//! Durable membership resolution and adapter reauthorization are exercised by
//! the `access::authority` and dispatch tests. These assertions consume the
//! exported production role templates instead of maintaining a second,
//! self-fulfilling authorization implementation in the test suite.

use labby_primitives::access::{Capability, CapabilitySchemaVersion, RoleTemplate};

const ALL: [Capability; 11] = [
    Capability::PlatformRead,
    Capability::PlatformManage,
    Capability::ScopeRead,
    Capability::ScopeOperate,
    Capability::ScopeCreate,
    Capability::ScopeManage,
    Capability::ScopeDelete,
    Capability::MembershipManage,
    Capability::OwnershipTransfer,
    Capability::PolicyExplain,
    Capability::AuditRead,
];

#[test]
fn required_product_roles_have_exact_production_capability_sets() {
    let cases: [(RoleTemplate, &[Capability]); 5] = [
        (RoleTemplate::PlatformAdmin, &ALL),
        (
            RoleTemplate::TeamOwner,
            &[
                Capability::ScopeRead,
                Capability::ScopeOperate,
                Capability::ScopeCreate,
                Capability::ScopeManage,
                Capability::ScopeDelete,
                Capability::MembershipManage,
                Capability::OwnershipTransfer,
                Capability::PolicyExplain,
                Capability::AuditRead,
            ],
        ),
        (
            RoleTemplate::TeamAdmin,
            &[
                Capability::ScopeRead,
                Capability::ScopeOperate,
                Capability::ScopeCreate,
                Capability::ScopeManage,
                Capability::MembershipManage,
                Capability::PolicyExplain,
                Capability::AuditRead,
            ],
        ),
        (
            RoleTemplate::TeamMember,
            &[
                Capability::ScopeRead,
                Capability::ScopeOperate,
                Capability::ScopeCreate,
            ],
        ),
        (
            RoleTemplate::PersonalUser,
            &[
                Capability::ScopeRead,
                Capability::ScopeOperate,
                Capability::ScopeCreate,
                Capability::ScopeManage,
                Capability::ScopeDelete,
            ],
        ),
    ];
    for (role, expected) in cases {
        let actual = role.capabilities(CapabilitySchemaVersion::V1).unwrap();
        for capability in ALL {
            assert_eq!(
                actual.contains(&capability),
                expected.contains(&capability),
                "unexpected {capability:?} decision for {role:?}"
            );
        }
    }
}

#[test]
fn production_wire_adapter_and_unknown_schema_fail_closed() {
    for capability in ALL {
        assert_eq!(
            Capability::from_wire(CapabilitySchemaVersion::V1, capability.as_wire()),
            Some(capability)
        );
    }
    assert_eq!(
        Capability::from_wire(CapabilitySchemaVersion::V1, "scope.superuser"),
        None
    );
    assert_eq!(
        Capability::from_wire(CapabilitySchemaVersion::new(2), "scope.read"),
        None
    );
    assert_eq!(
        RoleTemplate::PlatformAdmin.capabilities(CapabilitySchemaVersion::new(2)),
        None
    );
}

#[test]
fn production_templates_preserve_privilege_boundaries() {
    let capabilities = |role: RoleTemplate| role.capabilities(CapabilitySchemaVersion::V1).unwrap();
    assert!(capabilities(RoleTemplate::PlatformAdmin).contains(&Capability::PlatformManage));
    assert!(!capabilities(RoleTemplate::TeamOwner).contains(&Capability::PlatformManage));
    assert!(capabilities(RoleTemplate::TeamOwner).contains(&Capability::OwnershipTransfer));
    assert!(!capabilities(RoleTemplate::TeamAdmin).contains(&Capability::OwnershipTransfer));
    assert!(capabilities(RoleTemplate::TeamAdmin).contains(&Capability::MembershipManage));
    assert!(!capabilities(RoleTemplate::TeamMember).contains(&Capability::MembershipManage));
    assert!(capabilities(RoleTemplate::PersonalUser).contains(&Capability::ScopeDelete));
    assert!(!capabilities(RoleTemplate::PersonalUser).contains(&Capability::MembershipManage));
}
