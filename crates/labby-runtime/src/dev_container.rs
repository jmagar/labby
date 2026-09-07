//! Dev Container admission and lifecycle validation.
//!
//! This module performs no container-engine or filesystem work.

use std::collections::BTreeSet;

use labby_primitives::dev_container::{
    ApprovedTemplate, DesiredState, DevContainerQuota, HostCapability, LifecycleNonce,
    ObservedState,
};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchResources {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub lifetime_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchRequest {
    pub resources: LaunchResources,
    pub host_capabilities: BTreeSet<HostCapability>,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DevContainerAdmissionError {
    #[error("Dev Container resource request exceeds the approved template quota")]
    QuotaExceeded,
    #[error("Dev Container requested a host capability not approved by its template")]
    HostCapabilityDenied,
    #[error("Dev Container lifecycle nonce does not match the durable instance")]
    StaleLifecycleNonce,
    #[error("Dev Container lifecycle transition is invalid")]
    InvalidLifecycleTransition,
}

pub fn validate_launch(
    template: &ApprovedTemplate,
    request: &LaunchRequest,
) -> Result<(), DevContainerAdmissionError> {
    let ceiling = template.quota_ceiling();
    if request.resources.cpu_millis == 0
        || request.resources.memory_bytes == 0
        || request.resources.disk_bytes == 0
        || request.resources.lifetime_seconds == 0
        || request.resources.cpu_millis > ceiling.cpu_millis
        || request.resources.memory_bytes > ceiling.memory_bytes
        || request.resources.disk_bytes > ceiling.disk_bytes
        || request.resources.lifetime_seconds > ceiling.max_lifetime_seconds
    {
        return Err(DevContainerAdmissionError::QuotaExceeded);
    }
    if !template
        .host_capabilities()
        .allows_all(&request.host_capabilities)
    {
        return Err(DevContainerAdmissionError::HostCapabilityDenied);
    }
    Ok(())
}

pub fn validate_observation(
    durable_nonce: &LifecycleNonce,
    observed_nonce: &LifecycleNonce,
    desired: DesiredState,
    prior: ObservedState,
    next: ObservedState,
) -> Result<(), DevContainerAdmissionError> {
    if durable_nonce != observed_nonce {
        return Err(DevContainerAdmissionError::StaleLifecycleNonce);
    }
    let permitted = match (desired, prior, next) {
        (_, current, candidate) if current == candidate => true,
        (DesiredState::Running, ObservedState::Pending, ObservedState::Starting)
        | (DesiredState::Running, ObservedState::Starting, ObservedState::Running)
        | (
            DesiredState::Stopped | DesiredState::Deleted,
            ObservedState::Running | ObservedState::Starting,
            ObservedState::Stopping,
        )
        | (DesiredState::Stopped, ObservedState::Stopping, ObservedState::Stopped)
        | (
            DesiredState::Deleted,
            ObservedState::Stopped | ObservedState::Failed,
            ObservedState::Deleted,
        )
        | (
            _,
            ObservedState::Pending
            | ObservedState::Starting
            | ObservedState::Running
            | ObservedState::Stopping,
            ObservedState::Failed,
        ) => true,
        _ => false,
    };
    if permitted {
        Ok(())
    } else {
        Err(DevContainerAdmissionError::InvalidLifecycleTransition)
    }
}

pub fn active_quota_available(quota: DevContainerQuota, active: u32) -> bool {
    active < quota.max_active_instances
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_primitives::dev_container::{
        DevContainerTemplateId, HostCapabilityPolicy, ImageDigest,
    };

    fn template(policy: HostCapabilityPolicy) -> ApprovedTemplate {
        ApprovedTemplate::new(
            DevContainerTemplateId::new("rust-stable").unwrap(),
            ImageDigest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            DevContainerQuota {
                max_active_instances: 2,
                cpu_millis: 2_000,
                memory_bytes: 2_000,
                disk_bytes: 3_000,
                max_lifetime_seconds: 3_600,
            },
            policy,
        )
        .unwrap()
    }

    #[test]
    fn launch_requires_bounded_resources_and_explicit_host_capabilities() {
        let request = LaunchRequest {
            resources: LaunchResources {
                cpu_millis: 1_000,
                memory_bytes: 1_000,
                disk_bytes: 2_000,
                lifetime_seconds: 60,
            },
            host_capabilities: BTreeSet::from([HostCapability::HostNetwork]),
        };
        assert_eq!(
            validate_launch(&template(HostCapabilityPolicy::deny_all()), &request),
            Err(DevContainerAdmissionError::HostCapabilityDenied)
        );
        assert_eq!(
            validate_launch(
                &template(HostCapabilityPolicy::approved([
                    HostCapability::HostNetwork
                ])),
                &request
            ),
            Ok(())
        );
        let mut oversized = request;
        oversized.resources.memory_bytes = 2_001;
        assert_eq!(
            validate_launch(
                &template(HostCapabilityPolicy::approved([
                    HostCapability::HostNetwork
                ])),
                &oversized
            ),
            Err(DevContainerAdmissionError::QuotaExceeded)
        );
    }

    #[test]
    fn observations_are_nonce_bound_and_follow_the_state_machine() {
        let nonce = LifecycleNonce::new("11111111111111111111111111111111").unwrap();
        assert_eq!(
            validate_observation(
                &nonce,
                &LifecycleNonce::new("00000000000000000000000000000000").unwrap(),
                DesiredState::Running,
                ObservedState::Pending,
                ObservedState::Starting,
            ),
            Err(DevContainerAdmissionError::StaleLifecycleNonce)
        );
        assert_eq!(
            validate_observation(
                &nonce,
                &nonce,
                DesiredState::Running,
                ObservedState::Pending,
                ObservedState::Running,
            ),
            Err(DevContainerAdmissionError::InvalidLifecycleTransition)
        );
        assert!(
            validate_observation(
                &nonce,
                &nonce,
                DesiredState::Running,
                ObservedState::Pending,
                ObservedState::Starting,
            )
            .is_ok()
        );
    }

    #[test]
    fn active_instance_limit_is_a_strict_admission_ceiling() {
        let quota = template(HostCapabilityPolicy::deny_all()).quota_ceiling();
        assert!(active_quota_available(quota, 1));
        assert!(!active_quota_available(quota, 2));
        assert!(!active_quota_available(quota, 3));
    }
}
