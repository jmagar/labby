//! Asynchronous, bounded Labby authority projection into Depot.
#![allow(
    dead_code,
    reason = "constructed by optional Depot projection startup wiring"
)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use labby_primitives::authority_projection::{
    AUTHORITY_PROJECTION_SCHEMA_VERSION, AuthorityProjectionAck, AuthorityProjectionEnvelope,
    AuthorityProjectionRecord, MAX_AUTHORITY_ENVELOPE_BYTES, MAX_AUTHORITY_RECORDS_PER_BATCH,
    ProjectionKind,
};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::access::AccessStore;

const SEND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default)]
struct ProjectionHealth {
    managed: bool,
    ready: bool,
    pending: Option<String>,
    last_success: Option<Instant>,
    lag: usize,
    gap: bool,
    key_generation: Option<String>,
    watermark: Option<u64>,
}

#[cfg_attr(feature = "api-docs", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, serde::Serialize)]
pub struct ManagedProjectionReadiness {
    pub managed: bool,
    pub ready: bool,
    pub stale: bool,
    pub lag: usize,
    pub gap: bool,
    pub watermark: Option<u64>,
    pub key_generation: Option<String>,
    pub pending: Option<String>,
}

fn projection_health() -> &'static Mutex<ProjectionHealth> {
    static HEALTH: OnceLock<Mutex<ProjectionHealth>> = OnceLock::new();
    HEALTH.get_or_init(|| Mutex::new(ProjectionHealth::default()))
}

fn set_projection_health(managed: bool, ready: bool, pending: Option<&str>) {
    if let Ok(mut health) = projection_health().lock() {
        *health = ProjectionHealth {
            managed,
            ready,
            pending: pending.map(str::to_owned),
            last_success: ready.then(Instant::now),
            lag: usize::from(!ready),
            gap: false,
            key_generation: None,
            watermark: None,
        };
    }
}

pub(crate) fn projection_readiness() -> Option<ManagedProjectionReadiness> {
    projection_health().lock().ok().and_then(|health| {
        health.managed.then(|| {
            let stale = health
                .last_success
                .is_some_and(|success| success.elapsed() > Duration::from_secs(15));
            ManagedProjectionReadiness {
                managed: true,
                ready: health.ready && !stale && health.lag == 0 && !health.gap,
                stale,
                lag: health.lag,
                gap: health.gap,
                watermark: health.watermark,
                key_generation: health.key_generation.clone(),
                pending: health.pending.clone(),
            }
        })
    })
}

fn record_projection_ack(ack: &AuthorityProjectionAck) {
    if let Ok(mut health) = projection_health().lock() {
        health.watermark = Some(
            health
                .watermark
                .map_or(ack.highest_contiguous_sequence, |current| {
                    current.max(ack.highest_contiguous_sequence)
                }),
        );
    }
}

/// Returns a non-sensitive readiness reason while managed authority is stale.
pub(crate) fn managed_projection_readiness_pending() -> Option<String> {
    projection_health().lock().ok().and_then(|health| {
        let stale = health
            .last_success
            .is_some_and(|success| success.elapsed() > Duration::from_secs(15));
        (health.managed && (!health.ready || stale || health.lag > 0 || health.gap)).then(|| {
            health
                .pending
                .clone()
                .unwrap_or_else(|| "Depot authority projection is not ready".to_owned())
        })
    })
}

fn mark_projection_ready(key_id: &str) {
    if let Ok(mut health) = projection_health().lock() {
        health.managed = true;
        health.ready = true;
        health.pending = None;
        health.last_success = Some(Instant::now());
        health.lag = 0;
        health.gap = false;
        health.key_generation = Some(format!(
            "sha256:{}",
            hex::encode(Sha256::digest(key_id.as_bytes()))
        ));
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProjectionSendError {
    #[error("authority projection configuration is invalid")]
    Configuration,
    #[error("authority projection store is unavailable")]
    Store,
    #[error("authority projection transport is unavailable")]
    Transport,
    #[error("authority projection was rejected")]
    Rejected,
    #[error("authority projection response is invalid")]
    InvalidResponse,
}

#[derive(Clone)]
pub(crate) struct AuthorityProjectionSender {
    http: Client,
    endpoint: Url,
    readiness_endpoint: Url,
    bearer: Arc<str>,
    installation_id: Arc<str>,
    key_id: Arc<str>,
    signing_key: Arc<SigningKey>,
    store: AccessStore,
}

impl std::fmt::Debug for AuthorityProjectionSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorityProjectionSender")
            .field("endpoint", &self.endpoint)
            .field("installation_id", &self.installation_id)
            .field("key_id", &self.key_id)
            .field("credentials", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl AuthorityProjectionSender {
    pub(crate) fn new(
        base_url: Url,
        bearer: impl Into<Arc<str>>,
        installation_id: impl Into<Arc<str>>,
        key_id: impl Into<Arc<str>>,
        secret_key: [u8; 32],
        store: AccessStore,
    ) -> Result<Self, ProjectionSendError> {
        let endpoint = base_url
            .join("/api/authority/projection")
            .map_err(|_| ProjectionSendError::Configuration)?;
        let readiness_endpoint = base_url
            .join("/api/authority/readiness")
            .map_err(|_| ProjectionSendError::Configuration)?;
        let bearer = bearer.into();
        let installation_id = installation_id.into();
        let key_id = key_id.into();
        if bearer.is_empty() || installation_id.is_empty() || key_id.is_empty() {
            return Err(ProjectionSendError::Configuration);
        }
        Ok(Self {
            http: Client::builder()
                .timeout(SEND_TIMEOUT)
                .build()
                .map_err(|_| ProjectionSendError::Configuration)?,
            endpoint,
            readiness_endpoint,
            bearer,
            installation_id,
            key_id,
            signing_key: Arc::new(SigningKey::from_bytes(&secret_key)),
            store,
        })
    }

    pub(crate) async fn readiness(&self) -> Result<ProjectionReadiness, ProjectionSendError> {
        let response = self
            .http
            .get(self.readiness_endpoint.clone())
            .bearer_auth(self.bearer.as_ref())
            .send()
            .await
            .map_err(|_| ProjectionSendError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(ProjectionSendError::Rejected);
        }
        let readiness: ProjectionReadiness = response
            .json()
            .await
            .map_err(|_| ProjectionSendError::InvalidResponse)?;
        if !readiness.accepting.unwrap_or(readiness.ready) {
            return Err(ProjectionSendError::Rejected);
        }
        Ok(readiness)
    }

    /// Sends a caller-generated complete organization snapshot after binding it
    /// to Depot's durable watermark. A restored/older producer cannot roll the
    /// consumer backward because sequence and previous digest come from readiness.
    pub(crate) async fn send_snapshot(
        &self,
        organization_id: &str,
        records: Vec<AuthorityProjectionRecord>,
        now: i64,
    ) -> Result<AuthorityProjectionAck, ProjectionSendError> {
        let snapshot_id = snapshot_id(organization_id, now);
        self.send_snapshot_chunk(organization_id, records, now, true, &snapshot_id, true)
            .await
    }

    async fn send_snapshot_chunk(
        &self,
        organization_id: &str,
        mut records: Vec<AuthorityProjectionRecord>,
        now: i64,
        replace: bool,
        snapshot_id: &str,
        snapshot_complete: bool,
    ) -> Result<AuthorityProjectionAck, ProjectionSendError> {
        if records.is_empty() || records.len() > MAX_AUTHORITY_RECORDS_PER_BATCH {
            return Err(ProjectionSendError::Configuration);
        }
        let readiness = self.readiness().await?;
        let watermark = readiness.organizations.get(organization_id);
        let base = watermark.map_or(0, |value| value.highest_contiguous_sequence);
        for (offset, record) in records.iter_mut().enumerate() {
            record.sequence = base
                .checked_add(
                    u64::try_from(offset).map_err(|_| ProjectionSendError::Configuration)? + 1,
                )
                .ok_or(ProjectionSendError::Configuration)?;
        }
        let envelope = sign_envelope(
            self.installation_id.as_ref(),
            organization_id,
            ProjectionKind::Snapshot,
            replace.then_some(base),
            Some(snapshot_id),
            Some(snapshot_complete),
            rfc3339_timestamp(now)?,
            watermark.and_then(|value| value.last_envelope_digest.clone()),
            self.key_id.as_ref(),
            records,
            self.signing_key.as_ref(),
        )?;
        let body = serde_json::to_vec(&envelope).map_err(|_| ProjectionSendError::Configuration)?;
        if body.len() > MAX_AUTHORITY_ENVELOPE_BYTES {
            return Err(ProjectionSendError::Configuration);
        }
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(self.bearer.as_ref())
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| ProjectionSendError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(ProjectionSendError::Rejected);
        }
        let response: ProjectionResponse = response
            .json()
            .await
            .map_err(|_| ProjectionSendError::InvalidResponse)?;
        let ack = response.ack;
        let expected_final = base
            .checked_add(
                u64::try_from(envelope.records.len())
                    .map_err(|_| ProjectionSendError::Configuration)?,
            )
            .ok_or(ProjectionSendError::Configuration)?;
        if ack.organization_id != organization_id
            || ack.highest_contiguous_sequence != expected_final
        {
            return Err(ProjectionSendError::InvalidResponse);
        }
        Ok(ack)
    }

    /// Builds a typed snapshot from the current AccessStore state. This is the
    /// reconnect path; it does not reuse audit fingerprints as resource IDs.
    pub(crate) async fn send_current_snapshot(
        &self,
        organization_id: &str,
        now: i64,
    ) -> Result<(AuthorityProjectionAck, Option<u64>), ProjectionSendError> {
        let checkpoint = self
            .store
            .authority_snapshot_checkpoint(organization_id.to_owned())
            .await
            .map_err(|_| ProjectionSendError::Store)?;
        if checkpoint.record_count == 0 {
            return Err(ProjectionSendError::Configuration);
        }
        let file = checkpoint
            .spool
            .reopen()
            .map_err(|_| ProjectionSendError::Store)?;
        let mut lines = BufReader::new(file).lines();
        let snapshot_id = snapshot_id(organization_id, now);
        let chunk_count = checkpoint
            .record_count
            .div_ceil(MAX_AUTHORITY_RECORDS_PER_BATCH);
        let mut ack = None;
        for index in 0..chunk_count {
            let chunk = read_spooled_chunk(&mut lines)?;
            if chunk.is_empty() {
                return Err(ProjectionSendError::Store);
            }
            ack = self
                .send_snapshot_chunk(
                    organization_id,
                    chunk,
                    now,
                    index == 0,
                    &snapshot_id,
                    index + 1 == chunk_count,
                )
                .await?
                .into();
        }
        if lines.next().is_some() {
            return Err(ProjectionSendError::Store);
        }
        Ok((
            ack.ok_or(ProjectionSendError::Store)?,
            checkpoint.outbox_cutoff,
        ))
    }

    /// Performs at most one bounded delivery pass. It is intended for a supervised
    /// background loop; authorization and mutation responses never await this method.
    pub(crate) async fn send_once(&self, now: i64) -> Result<usize, ProjectionSendError> {
        let pending = self
            .store
            .claim_authority_projection_batch(now, MAX_AUTHORITY_RECORDS_PER_BATCH)
            .await
            .map_err(|_| ProjectionSendError::Store)?;
        if pending.is_empty() {
            return Ok(0);
        }
        let mut organizations: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for row in pending {
            organizations
                .entry(row.organization_id.clone())
                .or_default()
                .push(row);
        }
        let mut sent = 0;
        for (organization_id, rows) in organizations {
            let through = rows.last().map(|row| row.sequence).unwrap_or_default();
            let result = self.send_current_snapshot(&organization_id, now).await;
            match result {
                Ok((ack, Some(cutoff))) if ack.organization_id == organization_id => {
                    self.store
                        .supersede_authority_projection_with_snapshot(
                            organization_id,
                            ack.last_envelope_digest,
                            cutoff,
                            now,
                        )
                        .await
                        .map_err(|_| ProjectionSendError::Store)?;
                    sent += rows.len();
                }
                Ok(_) => {
                    self.store
                        .release_failed_authority_projection(organization_id, through, now)
                        .await
                        .map_err(|_| ProjectionSendError::Store)?;
                    return Err(ProjectionSendError::InvalidResponse);
                }
                Err(error) => {
                    self.store
                        .release_failed_authority_projection(organization_id, through, now)
                        .await
                        .map_err(|_| ProjectionSendError::Store)?;
                    return Err(error);
                }
            }
        }
        Ok(sent)
    }

    async fn send_delta(
        &self,
        organization_id: &str,
        rows: &[crate::access::PendingProjection],
        now: i64,
    ) -> Result<AuthorityProjectionAck, ProjectionSendError> {
        let current = self
            .store
            .authority_snapshot(organization_id.to_owned())
            .await
            .map_err(|_| ProjectionSendError::Store)?
            .into_iter()
            .map(|record| ((record.resource_type, record.resource_id), record.value))
            .collect::<BTreeMap<_, _>>();
        let records = rows
            .iter()
            .map(|row| {
                let value: Value = serde_json::from_str(&row.payload_json)
                    .map_err(|_| ProjectionSendError::Store)?;
                let resource_type = value
                    .get("resource_type")
                    .and_then(Value::as_str)
                    .ok_or(ProjectionSendError::Store)?
                    .to_owned();
                let resource_id = value
                    .get("resource_id")
                    .and_then(Value::as_str)
                    .ok_or(ProjectionSendError::Store)?
                    .to_owned();
                let operation = value
                    .get("operation")
                    .and_then(Value::as_str)
                    .ok_or(ProjectionSendError::Store)?
                    .to_owned();
                let authoritative = if operation == "delete" {
                    None
                } else if matches!(
                    resource_type.as_str(),
                    "principal" | "team" | "team_membership" | "team_project"
                ) {
                    Some(
                        current
                            .get(&(resource_type.clone(), resource_id.clone()))
                            .cloned()
                            .ok_or(ProjectionSendError::Store)?,
                    )
                } else {
                    value.get("value").cloned().filter(|value| !value.is_null())
                };
                Ok(AuthorityProjectionRecord {
                    sequence: row.sequence,
                    resource_type,
                    resource_id,
                    operation,
                    value: authoritative,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let previous_digest = rows.first().and_then(|row| row.previous_digest.clone());
        let generated_at = rfc3339_timestamp(now)?;
        let envelope = sign_envelope(
            self.installation_id.as_ref(),
            organization_id,
            ProjectionKind::Delta,
            None,
            None,
            None,
            generated_at,
            previous_digest,
            self.key_id.as_ref(),
            records,
            self.signing_key.as_ref(),
        )?;
        let body = serde_json::to_vec(&envelope).map_err(|_| ProjectionSendError::Configuration)?;
        if body.len() > MAX_AUTHORITY_ENVELOPE_BYTES {
            return Err(ProjectionSendError::Configuration);
        }
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(self.bearer.as_ref())
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| ProjectionSendError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(ProjectionSendError::Rejected);
        }
        let response: ProjectionResponse = response
            .json()
            .await
            .map_err(|_| ProjectionSendError::InvalidResponse)?;
        record_projection_ack(&response.ack);
        Ok(response.ack)
    }
}

pub(crate) async fn start_managed_projection(
    preferences: &crate::config::depot::DepotPreferences,
) -> Result<Option<tokio::task::JoinHandle<()>>, ProjectionSendError> {
    use crate::config::depot::DepotControlMode;

    if preferences.control_mode != DepotControlMode::LabbyManaged
        || preferences.managed_authority_kill_switch
    {
        set_projection_health(false, true, None);
        return Ok(None);
    }
    set_projection_health(
        true,
        false,
        Some("Depot authority projection is initializing"),
    );
    let endpoint = preferences
        .authority_endpoint
        .as_deref()
        .ok_or(ProjectionSendError::Configuration)?;
    let bearer_env = preferences
        .authority_bearer_token_env
        .as_deref()
        .filter(|name| crate::config::depot::allowed_secret_reference(name))
        .ok_or(ProjectionSendError::Configuration)?;
    let signing_env = preferences
        .authority_signing_key_env
        .as_deref()
        .filter(|name| crate::config::depot::allowed_secret_reference(name))
        .ok_or(ProjectionSendError::Configuration)?;
    let installation_id = preferences
        .authority_installation_id
        .as_deref()
        .ok_or(ProjectionSendError::Configuration)?;
    let key_id = preferences
        .authority_key_id
        .as_deref()
        .ok_or(ProjectionSendError::Configuration)?;
    let bearer = std::env::var(bearer_env).map_err(|_| ProjectionSendError::Configuration)?;
    let encoded_key = std::env::var(signing_env).map_err(|_| ProjectionSendError::Configuration)?;
    let key_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded_key)
        .map_err(|_| ProjectionSendError::Configuration)?;
    let secret_key: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| ProjectionSendError::Configuration)?;
    let path = crate::config::access_db_path().map_err(|_| ProjectionSendError::Configuration)?;
    let store = AccessStore::open_existing_current(path)
        .await
        .map_err(|_| ProjectionSendError::Store)?;
    let sender = AuthorityProjectionSender::new(
        Url::parse(endpoint).map_err(|_| ProjectionSendError::Configuration)?,
        bearer,
        installation_id,
        key_id,
        secret_key,
        store,
    )?;

    Ok(Some(tokio::spawn(async move {
        let mut loop_state = ProjectionLoopState::needs_snapshot();
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if loop_state.next_work() == ProjectionWork::Baseline {
                match synchronize_baseline(&sender, unix_now()).await {
                    Ok(()) => {
                        loop_state.baseline_finished(true);
                        match sender.send_once(unix_now()).await {
                            Ok(_) => mark_projection_ready(sender.key_id.as_ref()),
                            Err(error) => {
                                set_projection_health(
                                    true,
                                    false,
                                    Some("Depot authority projection delivery failed"),
                                );
                                tracing::warn!(error = %error, "Depot authority projection delivery failed after baseline synchronization");
                            }
                        }
                    }
                    Err(error) => {
                        loop_state.baseline_finished(false);
                        set_projection_health(
                            true,
                            false,
                            Some("Depot authority projection initial synchronization failed"),
                        );
                        tracing::warn!(error = %error, "Depot authority baseline synchronization failed; full snapshot will be retried");
                    }
                }
                continue;
            }

            match sender.send_once(unix_now()).await {
                Ok(_) => mark_projection_ready(sender.key_id.as_ref()),
                Err(error) => {
                    set_projection_health(
                        true,
                        false,
                        Some("Depot authority projection delivery failed"),
                    );
                    tracing::warn!(error = %error, "Depot authority projection delivery failed");
                }
            }
        }
    })))
}

async fn synchronize_baseline(
    sender: &AuthorityProjectionSender,
    now: i64,
) -> Result<(), ProjectionSendError> {
    let organizations = sender.store.authority_organizations().await.map_err(|_| {
        tracing::warn!("could not enumerate authority organizations");
        ProjectionSendError::Store
    })?;
    if organizations.is_empty() {
        return Err(ProjectionSendError::Store);
    }

    for organization in organizations {
        match sender.send_current_snapshot(&organization, now).await? {
            (ack, Some(cutoff)) => {
                record_projection_ack(&ack);
                sender
                    .store
                    .supersede_authority_projection_with_snapshot(
                        organization,
                        ack.last_envelope_digest,
                        cutoff,
                        now,
                    )
                    .await
                    .map_err(|error| {
                        tracing::warn!(error = %error, "could not checkpoint initial Depot authority snapshot");
                        ProjectionSendError::Store
                    })?;
            }
            (ack, None) => record_projection_ack(&ack),
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectionWork {
    Baseline,
    Deltas,
}

#[derive(Clone, Copy, Debug)]
struct ProjectionLoopState {
    needs_snapshot: bool,
}

impl ProjectionLoopState {
    const fn needs_snapshot() -> Self {
        Self {
            needs_snapshot: true,
        }
    }

    const fn next_work(self) -> ProjectionWork {
        if self.needs_snapshot {
            ProjectionWork::Baseline
        } else {
            ProjectionWork::Deltas
        }
    }

    const fn baseline_finished(&mut self, accepted_and_checkpointed: bool) {
        self.needs_snapshot = !accepted_and_checkpointed;
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ProjectionReadiness {
    pub(crate) ready: bool,
    #[serde(default)]
    pub(crate) accepting: Option<bool>,
    #[serde(default)]
    pub(crate) organizations: BTreeMap<String, ProjectionWatermark>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ProjectionWatermark {
    pub(crate) highest_contiguous_sequence: u64,
    pub(crate) last_envelope_digest: Option<String>,
}

#[derive(Deserialize)]
struct ProjectionResponse {
    ack: AuthorityProjectionAck,
}

#[derive(Deserialize)]
struct SpooledAuthorityRecord {
    resource_type: String,
    resource_id: String,
    value: Value,
}

fn read_spooled_chunk(
    lines: &mut impl Iterator<Item = std::io::Result<String>>,
) -> Result<Vec<AuthorityProjectionRecord>, ProjectionSendError> {
    let mut chunk = Vec::with_capacity(MAX_AUTHORITY_RECORDS_PER_BATCH);
    while chunk.len() < MAX_AUTHORITY_RECORDS_PER_BATCH {
        let Some(line) = lines.next() else { break };
        let record: SpooledAuthorityRecord =
            serde_json::from_str(&line.map_err(|_| ProjectionSendError::Store)?)
                .map_err(|_| ProjectionSendError::Store)?;
        chunk.push(AuthorityProjectionRecord {
            sequence: 0,
            resource_type: record.resource_type,
            resource_id: record.resource_id,
            operation: "upsert".into(),
            value: Some(record.value),
        });
    }
    Ok(chunk)
}

#[derive(Serialize)]
struct UnsignedEnvelope<'a> {
    schema_version: u16,
    installation_id: &'a str,
    organization_id: &'a str,
    sequence_start: u64,
    sequence_end: u64,
    kind: ProjectionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_base_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_complete: Option<bool>,
    generated_at: &'a str,
    previous_digest: &'a Option<String>,
    payload_digest: &'a str,
    key_id: &'a str,
    records: &'a [AuthorityProjectionRecord],
}

fn sign_envelope(
    installation_id: &str,
    organization_id: &str,
    kind: ProjectionKind,
    snapshot_base_sequence: Option<u64>,
    snapshot_id: Option<&str>,
    snapshot_complete: Option<bool>,
    generated_at: String,
    previous_digest: Option<String>,
    key_id: &str,
    records: Vec<AuthorityProjectionRecord>,
    key: &SigningKey,
) -> Result<AuthorityProjectionEnvelope, ProjectionSendError> {
    let sequence_start = records
        .first()
        .map(|r| r.sequence)
        .ok_or(ProjectionSendError::Configuration)?;
    let sequence_end = records
        .last()
        .map(|r| r.sequence)
        .ok_or(ProjectionSendError::Configuration)?;
    let records_bytes = canonical_json(
        &serde_json::to_value(&records).map_err(|_| ProjectionSendError::Configuration)?,
    )?;
    let payload_digest = format!("sha256:{}", hex::encode(Sha256::digest(&records_bytes)));
    let unsigned = UnsignedEnvelope {
        schema_version: AUTHORITY_PROJECTION_SCHEMA_VERSION,
        installation_id,
        organization_id,
        sequence_start,
        sequence_end,
        kind,
        snapshot_base_sequence,
        snapshot_id,
        snapshot_complete,
        generated_at: &generated_at,
        previous_digest: &previous_digest,
        payload_digest: &payload_digest,
        key_id,
        records: &records,
    };
    let signing_bytes = canonical_json(
        &serde_json::to_value(unsigned).map_err(|_| ProjectionSendError::Configuration)?,
    )?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(key.sign(&signing_bytes).to_bytes());
    Ok(AuthorityProjectionEnvelope {
        schema_version: AUTHORITY_PROJECTION_SCHEMA_VERSION,
        installation_id: installation_id.into(),
        organization_id: organization_id.into(),
        sequence_start,
        sequence_end,
        kind,
        snapshot_base_sequence,
        snapshot_id: snapshot_id.map(str::to_owned),
        snapshot_complete,
        generated_at,
        previous_digest,
        payload_digest,
        key_id: key_id.into(),
        records,
        signature,
    })
}

fn snapshot_id(organization_id: &str, now: i64) -> String {
    let mut digest = Sha256::new();
    digest.update(organization_id.as_bytes());
    digest.update(now.to_be_bytes());
    format!("snapshot-{}", hex::encode(digest.finalize()))
}

fn rfc3339_timestamp(epoch_seconds: i64) -> Result<String, ProjectionSendError> {
    if epoch_seconds < 0 {
        return Err(ProjectionSendError::Configuration);
    }
    let days = epoch_seconds.div_euclid(86_400);
    let seconds = epoch_seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    if !(0..=9_999).contains(&year) {
        return Err(ProjectionSendError::Configuration);
    }
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

fn canonical_json(value: &Value) -> Result<Vec<u8>, ProjectionSendError> {
    fn normalize(value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), normalize(value)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.iter().map(normalize).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_vec(&normalize(value)).map_err(|_| ProjectionSendError::Configuration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_projection_health_fails_closed_until_synchronized() {
        set_projection_health(true, false, Some("projection lag is nonzero"));
        assert_eq!(
            managed_projection_readiness_pending().as_deref(),
            Some("projection lag is nonzero")
        );
        set_projection_health(true, true, None);
        assert!(managed_projection_readiness_pending().is_none());
        set_projection_health(false, true, None);
    }

    #[test]
    fn transient_baseline_rejection_cannot_fall_through_to_empty_deltas() {
        let mut state = ProjectionLoopState::needs_snapshot();
        assert_eq!(state.next_work(), ProjectionWork::Baseline);

        // A transient Depot rejection keeps the loop on full snapshots. The
        // empty-outbox delta path is therefore not eligible to mark Ready.
        state.baseline_finished(false);
        assert_eq!(state.next_work(), ProjectionWork::Baseline);

        // Only an accepted and locally checkpointed baseline unlocks deltas.
        state.baseline_finished(true);
        assert_eq!(state.next_work(), ProjectionWork::Deltas);
    }

    #[test]
    fn epoch_seconds_are_encoded_as_depot_parseable_rfc3339() {
        assert_eq!(rfc3339_timestamp(0).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339_timestamp(951_782_400).unwrap(),
            "2000-02-29T00:00:00Z"
        );
        assert!(rfc3339_timestamp(-1).is_err());
    }
    #[test]
    fn spooled_snapshots_never_materialize_more_than_one_envelope() {
        let count = MAX_AUTHORITY_RECORDS_PER_BATCH * 8 + 1;
        let mut lines = (0..count).map(|sequence| {
            Ok::<_, std::io::Error>(
                serde_json::json!({
                    "resource_type":"principal",
                    "resource_id":format!("principal-{sequence}"),
                    "value":{"status":"active"}
                })
                .to_string(),
            )
        });
        let mut total = 0;
        loop {
            let chunk = read_spooled_chunk(&mut lines).unwrap();
            assert!(chunk.len() <= MAX_AUTHORITY_RECORDS_PER_BATCH);
            total += chunk.len();
            if chunk.is_empty() {
                break;
            }
        }
        assert_eq!(total, count);
    }

    #[test]
    fn canonical_signing_is_stable_and_omits_signature() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let record = AuthorityProjectionRecord {
            sequence: 1,
            resource_type: "team".into(),
            resource_id: "t1".into(),
            operation: "upsert".into(),
            value: Some(serde_json::json!({"z":1,"a":2})),
        };
        let one = sign_envelope(
            "install",
            "org",
            ProjectionKind::Delta,
            None,
            None,
            None,
            "2026-09-05T00:00:00Z".into(),
            None,
            "key",
            vec![record.clone()],
            &key,
        )
        .unwrap();
        let two = sign_envelope(
            "install",
            "org",
            ProjectionKind::Delta,
            None,
            None,
            None,
            "2026-09-05T00:00:00Z".into(),
            None,
            "key",
            vec![record],
            &key,
        )
        .unwrap();
        assert_eq!(one, two);
        assert!(one.payload_digest.starts_with("sha256:"));
        assert!(!one.signature.contains('='));
    }

    #[test]
    fn canonical_json_matches_depot_golden_fixture() {
        let value = serde_json::json!({"sequence_start":1,"records":[],"previous_digest":null,"payload_digest":"sha256:placeholder","organization_id":"org-1","kind":"delta","key_id":"key-1","installation_id":"install-1","generated_at":"2026-09-05T00:00:00Z","sequence_end":1,"schema_version":1});
        assert_eq!(
            String::from_utf8(canonical_json(&value).unwrap()).unwrap(),
            "{\"generated_at\":\"2026-09-05T00:00:00Z\",\"installation_id\":\"install-1\",\"key_id\":\"key-1\",\"kind\":\"delta\",\"organization_id\":\"org-1\",\"payload_digest\":\"sha256:placeholder\",\"previous_digest\":null,\"records\":[],\"schema_version\":1,\"sequence_end\":1,\"sequence_start\":1}"
        );
    }

    #[test]
    fn projection_response_requires_the_ack_wrapper() {
        let wrapped = serde_json::json!({"ack": {
            "organization_id": "org-1",
            "highest_contiguous_sequence": 4,
            "last_envelope_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "snapshot_digest": null
        }});
        let response: ProjectionResponse = serde_json::from_value(wrapped).unwrap();
        assert_eq!(response.ack.organization_id, "org-1");
        assert!(
            serde_json::from_value::<ProjectionResponse>(serde_json::json!({
                "organization_id": "org-1",
                "highest_contiguous_sequence": 4,
                "last_envelope_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "snapshot_digest": null
            }))
            .is_err()
        );
    }
}
