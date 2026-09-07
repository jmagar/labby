//! Bounded server-side transport for the optional Depot control plane.

pub mod admin;
#[cfg(test)]
mod admin_tests;
pub(crate) mod authority_projection;
pub mod cursor;
#[cfg(test)]
mod cursor_tests;
pub mod discovery;
#[cfg(test)]
mod discovery_tests;
pub mod health;
pub mod manager;
#[cfg(test)]
mod manager_tests;
pub mod network;
#[cfg(test)]
mod network_tests;
pub mod operations;
pub mod provider;
pub mod scheduler;
#[cfg(test)]
mod scheduler_tests;
pub mod store;
#[cfg(test)]
mod store_tests;

use std::{collections::HashMap, env, sync::Arc, time::Duration};

use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const QUEUE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_INTERACTIVE_REQUESTS: usize = 16;
const COMPATIBILITY_SCHEMA_VERSION: &str = "labby.depot-compatibility/v1";

#[derive(Clone)]
pub struct DepotClient {
    http: Client,
    base_url: Option<Url>,
    token: Option<Arc<str>>,
    enabled: bool,
    interactive: Arc<Semaphore>,
    destructive_requests: Arc<Mutex<HashMap<String, DestructiveRequest>>>,
    operation_catalogs: Arc<Mutex<HashMap<String, OperationCatalogSnapshot>>>,
    queue_timeout: Duration,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepotStatus {
    pub configured: bool,
    pub enabled: bool,
    pub authority: DepotAuthority,
    pub max_response_bytes: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DepotAuthority {
    Unknown,
    Read,
    Write,
}

#[derive(Clone, Debug)]
enum DestructiveRequest {
    Pending([u8; 32], tokio::time::Instant),
    Complete([u8; 32], Value, tokio::time::Instant),
    Indeterminate([u8; 32], tokio::time::Instant),
}

impl DestructiveRequest {
    fn observed_at(&self) -> tokio::time::Instant {
        match self {
            Self::Pending(_, at) | Self::Complete(_, _, at) | Self::Indeterminate(_, at) => *at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationPolicy {
    pub read_only: bool,
    pub destructive: bool,
}

struct OperationCatalogSnapshot {
    observed_at: tokio::time::Instant,
    policies: HashMap<String, OperationPolicy>,
}

#[derive(Debug)]
pub enum DepotError {
    Disabled,
    Unconfigured,
    UnsupportedOperation,
    InvalidCatalog,
    DestructiveIntentRequired,
    IdempotencyConflict,
    OutcomeIndeterminate,
    Upstream(StatusCode, Value),
    QueueTimeout,
    Unavailable(TransportFailure),
    ResponseTooLarge,
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportFailure {
    Connect,
    Timeout,
    Request,
    ResponseBody,
}

impl TransportFailure {
    const fn category(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Timeout => "timeout",
            Self::Request => "request",
            Self::ResponseBody => "response_body",
        }
    }
}

impl DepotClient {
    /// Inert compatibility facade until the browser adapters receive the manager.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            http: Client::new(),
            base_url: None,
            token: None,
            enabled: false,
            interactive: Arc::new(Semaphore::new(MAX_INTERACTIVE_REQUESTS)),
            destructive_requests: Arc::new(Mutex::new(HashMap::new())),
            operation_catalogs: Arc::new(Mutex::new(HashMap::new())),
            queue_timeout: QUEUE_TIMEOUT,
        }
    }
    #[must_use]
    pub fn from_env() -> Self {
        let enabled = env::var("LABBY_DEPOT_ENABLED").is_ok_and(|value| value == "1");
        let base_url =
            env::var("LABBY_DEPOT_URL")
                .ok()
                .and_then(|value| match parse_base_url(&value) {
                    Ok(url) => Some(url),
                    Err(_) => {
                        tracing::warn!(
                            category = "invalid_base_url",
                            variable = "LABBY_DEPOT_URL",
                            "Depot configuration rejected"
                        );
                        None
                    }
                });
        let token = env::var("LABBY_DEPOT_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(Arc::from);
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Depot HTTP client configuration is valid");
        Self {
            http,
            base_url,
            token,
            enabled,
            interactive: Arc::new(Semaphore::new(MAX_INTERACTIVE_REQUESTS)),
            destructive_requests: Arc::new(Mutex::new(HashMap::new())),
            operation_catalogs: Arc::new(Mutex::new(HashMap::new())),
            queue_timeout: QUEUE_TIMEOUT,
        }
    }

    #[must_use]
    pub fn status(&self) -> DepotStatus {
        let configured = self.base_url.is_some() && self.token.is_some();
        DepotStatus {
            configured,
            enabled: self.enabled,
            // Configuration proves only that Labby can attempt a request. Depot
            // remains authoritative for token scopes, so local state must not
            // claim write authority that Depot has not attested.
            authority: DepotAuthority::Unknown,
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(base_url: Url, token: &str) -> Self {
        drop(rustls::crypto::ring::default_provider().install_default());
        Self {
            http: Client::builder().no_proxy().build().unwrap(),
            base_url: Some(base_url),
            token: Some(Arc::from(token)),
            enabled: true,
            interactive: Arc::new(Semaphore::new(MAX_INTERACTIVE_REQUESTS)),
            destructive_requests: Arc::new(Mutex::new(HashMap::new())),
            operation_catalogs: Arc::new(Mutex::new(HashMap::new())),
            queue_timeout: QUEUE_TIMEOUT,
        }
    }

    pub async fn session(&self, actor: &str) -> Result<Value, DepotError> {
        self.request(reqwest::Method::GET, "api/session", None, actor)
            .await
    }

    pub async fn status_for_actor(&self, actor: &str) -> DepotStatus {
        let mut status = self.status();
        if let Ok(catalog) = self.operations(actor).await
            && let Ok(catalog) = serde_json::from_value::<OperationCatalog>(catalog)
        {
            status.authority = if catalog
                .operations
                .iter()
                .any(|operation| !operation.annotations.read_only_hint)
            {
                DepotAuthority::Write
            } else {
                DepotAuthority::Read
            };
        }
        status
    }

    pub async fn operations(&self, actor: &str) -> Result<Value, DepotError> {
        let mut value = self
            .request(reqwest::Method::GET, "api/operations", None, actor)
            .await?;
        let policies = parse_operation_catalog(&value)?;
        project_operation_groups(&mut value)?;
        self.operation_catalogs.lock().await.insert(
            actor.to_owned(),
            OperationCatalogSnapshot {
                observed_at: tokio::time::Instant::now(),
                policies,
            },
        );
        Ok(value)
    }

    /// Resolve an operation solely from Depot's current actor-filtered catalog.
    pub async fn operation_policy(
        &self,
        operation: &str,
        actor: &str,
    ) -> Result<OperationPolicy, DepotError> {
        if !valid_operation_name(operation) {
            return Err(DepotError::UnsupportedOperation);
        }
        let catalogs = self.operation_catalogs.lock().await;
        let catalog = catalogs.get(actor).ok_or(DepotError::InvalidCatalog)?;
        if catalog.observed_at.elapsed() > Duration::from_mins(5) {
            return Err(DepotError::InvalidCatalog);
        }
        catalog
            .policies
            .get(operation)
            .copied()
            .ok_or(DepotError::UnsupportedOperation)
    }

    pub async fn call(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
        policy: OperationPolicy,
        idempotency_key: Option<&str>,
    ) -> Result<Value, DepotError> {
        if policy.destructive {
            let key = idempotency_key
                .filter(|key| valid_idempotency_key(key))
                .ok_or(DepotError::DestructiveIntentRequired)?;
            return self.call_destructive(operation, params, actor, key).await;
        }
        self.call_upstream(operation, params, actor, None).await
    }

    async fn call_destructive(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
        idempotency_key: &str,
    ) -> Result<Value, DepotError> {
        let digest: [u8; 32] = Sha256::digest(
            serde_json::to_vec(&json!({"actor":actor,"operation":operation,"params":params}))
                .map_err(|_| DepotError::InvalidResponse)?,
        )
        .into();
        {
            let mut requests = self.destructive_requests.lock().await;
            match requests.get(idempotency_key) {
                Some(DestructiveRequest::Complete(existing, value, _)) if existing == &digest => {
                    return Ok(value.clone());
                }
                Some(DestructiveRequest::Pending(existing, _)) if existing == &digest => {
                    return Err(DepotError::OutcomeIndeterminate);
                }
                Some(DestructiveRequest::Indeterminate(existing, _)) if existing == &digest => {
                    return Err(DepotError::OutcomeIndeterminate);
                }
                Some(_) => return Err(DepotError::IdempotencyConflict),
                None => {
                    let now = tokio::time::Instant::now();
                    if requests.len() >= 1024 {
                        requests.retain(|_, state| {
                            now.duration_since(state.observed_at()) < Duration::from_hours(24)
                        });
                    }
                    if requests.len() >= 1024 {
                        let evict = requests.iter().find_map(|(key, state)| {
                            (!matches!(state, DestructiveRequest::Pending(_, _)))
                                .then(|| key.clone())
                        });
                        if let Some(evict) = evict {
                            requests.remove(&evict);
                        } else {
                            return Err(DepotError::QueueTimeout);
                        }
                    }
                    requests.insert(
                        idempotency_key.to_owned(),
                        DestructiveRequest::Pending(digest, now),
                    );
                }
            }
        }
        let result = self
            .call_upstream(operation, params, actor, Some(idempotency_key))
            .await;
        let mut requests = self.destructive_requests.lock().await;
        match &result {
            Ok(value) => {
                requests.insert(
                    idempotency_key.to_owned(),
                    DestructiveRequest::Complete(
                        digest,
                        value.clone(),
                        tokio::time::Instant::now(),
                    ),
                );
            }
            Err(DepotError::Upstream(_, _)) => {
                requests.remove(idempotency_key);
            }
            Err(_) => {
                requests.insert(
                    idempotency_key.to_owned(),
                    DestructiveRequest::Indeterminate(digest, tokio::time::Instant::now()),
                );
            }
        }
        result
    }

    async fn call_upstream(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
        idempotency_key: Option<&str>,
    ) -> Result<Value, DepotError> {
        self.request_with_idempotency(
            reqwest::Method::POST,
            &format!("api/operations/{operation}"),
            Some(params),
            actor,
            idempotency_key,
        )
        .await
        .and_then(compatibility_envelope)
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        actor: &str,
    ) -> Result<Value, DepotError> {
        self.request_with_idempotency(method, path, body, actor, None)
            .await
    }

    async fn request_with_idempotency(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        actor: &str,
        idempotency_key: Option<&str>,
    ) -> Result<Value, DepotError> {
        if !self.enabled {
            return Err(DepotError::Disabled);
        }
        let _permit = tokio::time::timeout(self.queue_timeout, self.interactive.acquire())
            .await
            .map_err(|_| {
                tracing::warn!(category = "queue_timeout", "Depot request rejected");
                DepotError::QueueTimeout
            })?
            .map_err(|_| DepotError::Unavailable(TransportFailure::Request))?;
        let base = self.base_url.as_ref().ok_or(DepotError::Unconfigured)?;
        let token = self.token.as_ref().ok_or(DepotError::Unconfigured)?;
        let url = base.join(path).map_err(|_| DepotError::Unconfigured)?;
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(token.as_ref())
            .header("accept", "application/json")
            .header("x-labby-actor", actor);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(key) = idempotency_key {
            request = request.header("idempotency-key", key);
        }
        let response = request.send().await.map_err(|error| {
            let category = if error.is_timeout() {
                TransportFailure::Timeout
            } else if error.is_connect() {
                TransportFailure::Connect
            } else {
                TransportFailure::Request
            };
            tracing::warn!(category = category.category(), "Depot transport failed");
            DepotError::Unavailable(category)
        })?;
        decode_response(response).await
    }
}

fn compatibility_envelope(value: Value) -> Result<Value, DepotError> {
    let Value::Object(mut response) = value else {
        return Err(DepotError::InvalidResponse);
    };
    response.insert(
        "schemaVersion".to_string(),
        Value::String(COMPATIBILITY_SCHEMA_VERSION.to_string()),
    );
    response.insert("contractVersion".to_string(), Value::from(1));
    Ok(Value::Object(response))
}

#[derive(Deserialize)]
struct OperationCatalog {
    operations: Vec<CatalogOperation>,
}

#[derive(Deserialize)]
struct CatalogOperation {
    name: String,
    annotations: CatalogAnnotations,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogAnnotations {
    read_only_hint: bool,
    destructive_hint: bool,
}

#[cfg(test)]
fn parse_operation_policy(catalog: &Value, name: &str) -> Result<OperationPolicy, DepotError> {
    parse_operation_catalog(catalog)?
        .remove(name)
        .ok_or(DepotError::UnsupportedOperation)
}

fn parse_operation_catalog(
    catalog: &Value,
) -> Result<HashMap<String, OperationPolicy>, DepotError> {
    let catalog: OperationCatalog =
        serde_json::from_value(catalog.clone()).map_err(|_| DepotError::InvalidCatalog)?;
    let mut policies = HashMap::with_capacity(catalog.operations.len());
    for item in catalog.operations {
        if !valid_operation_name(&item.name)
            || item.annotations.read_only_hint && item.annotations.destructive_hint
        {
            return Err(DepotError::InvalidCatalog);
        }
        let policy = OperationPolicy {
            read_only: item.annotations.read_only_hint,
            destructive: item.annotations.destructive_hint,
        };
        if policies.insert(item.name, policy).is_some() {
            return Err(DepotError::InvalidCatalog);
        }
    }
    Ok(policies)
}

fn valid_operation_name(operation: &str) -> bool {
    !operation.is_empty()
        && operation.len() <= 256
        && operation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_idempotency_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 160 && key.bytes().all(|byte| byte.is_ascii_graphic())
}

fn project_operation_groups(value: &mut Value) -> Result<(), DepotError> {
    let operations = value
        .get_mut("operations")
        .and_then(Value::as_array_mut)
        .ok_or(DepotError::InvalidCatalog)?;
    for operation in operations {
        let object = operation
            .as_object_mut()
            .ok_or(DepotError::InvalidCatalog)?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or(DepotError::InvalidCatalog)?;
        let group = if name.starts_with("depot.tokens.") {
            "access"
        } else if name.starts_with("depot.maintenance.")
            || name.starts_with("depot.ingest.")
            || name.starts_with("depot.uploads.")
            || name.starts_with("depot.system.")
        {
            "operations"
        } else {
            "catalog"
        };
        object.insert("group".to_owned(), Value::String(group.to_owned()));
    }
    Ok(())
}

fn parse_base_url(value: &str) -> Result<Url, ()> {
    let url = Url::parse(&format!("{}/", value.trim_end_matches('/'))).map_err(|_| ())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    Ok(url)
}

async fn decode_response(mut response: reqwest::Response) -> Result<Value, DepotError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(DepotError::ResponseTooLarge);
    }
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        tracing::warn!(category = "response_body", "Depot transport failed");
        DepotError::Unavailable(TransportFailure::ResponseBody)
    })? {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(DepotError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| DepotError::InvalidResponse)?;
    if status.is_success() {
        Ok(value)
    } else {
        Err(DepotError::Upstream(status, value))
    }
}

pub fn error_body(error: &DepotError) -> Value {
    match error {
        DepotError::Upstream(status, _) => {
            json!({"error":"depot_rejected","status":status.as_u16()})
        }
        DepotError::Disabled => json!({"error":"depot_disabled"}),
        DepotError::Unconfigured => json!({"error":"depot_unconfigured"}),
        DepotError::UnsupportedOperation => json!({"error":"unsupported_operation"}),
        DepotError::InvalidCatalog => json!({"error":"invalid_depot_catalog"}),
        DepotError::DestructiveIntentRequired => json!({"error":"destructive_intent_required"}),
        DepotError::IdempotencyConflict => json!({"error":"idempotency_conflict"}),
        DepotError::OutcomeIndeterminate => {
            json!({"error":"outcome_indeterminate","recovery":{"action":"reconcile_before_retry"}})
        }
        DepotError::QueueTimeout => json!({"error":"depot_busy"}),
        DepotError::Unavailable(_) => json!({"error":"depot_unavailable"}),
        DepotError::ResponseTooLarge => json!({"error":"depot_response_too_large"}),
        DepotError::InvalidResponse => json!({"error":"invalid_depot_response"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client(base_url: Url, permits: usize, queue_timeout: Duration) -> DepotClient {
        drop(rustls::crypto::ring::default_provider().install_default());
        DepotClient {
            http: Client::builder()
                // Loopback refusal can outlast 250 ms on Windows. This fixture
                // verifies connect classification, not the request timeout.
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .build()
                .unwrap(),
            base_url: Some(base_url),
            token: Some(Arc::from("test-token")),
            enabled: true,
            interactive: Arc::new(Semaphore::new(permits)),
            destructive_requests: Arc::new(Mutex::new(HashMap::new())),
            operation_catalogs: Arc::new(Mutex::new(HashMap::new())),
            queue_timeout,
        }
    }

    #[test]
    fn operation_policy_comes_from_the_actor_filtered_catalog() {
        let catalog = json!({"operations":[
            {"name":"depot.new.read","annotations":{"readOnlyHint":true,"destructiveHint":false}},
            {"name":"depot.new.destroy","annotations":{"readOnlyHint":false,"destructiveHint":true}}
        ]});
        assert_eq!(
            parse_operation_policy(&catalog, "depot.new.read").unwrap(),
            OperationPolicy {
                read_only: true,
                destructive: false
            }
        );
        assert_eq!(
            parse_operation_policy(&catalog, "depot.new.destroy").unwrap(),
            OperationPolicy {
                read_only: false,
                destructive: true
            }
        );
        assert!(matches!(
            parse_operation_policy(&catalog, "depot.hidden"),
            Err(DepotError::UnsupportedOperation)
        ));
        assert!(matches!(
            parse_operation_policy(
                &json!({"operations":[{"name":"depot.bad","annotations":{}}]}),
                "depot.bad"
            ),
            Err(DepotError::InvalidCatalog)
        ));
    }

    #[test]
    fn operation_groups_are_projected_by_the_server() {
        let mut catalog = json!({"operations":[
            {"name":"depot.skills.list","annotations":{"readOnlyHint":true,"destructiveHint":false}},
            {"name":"depot.tokens.list","annotations":{"readOnlyHint":false,"destructiveHint":false}},
            {"name":"depot.maintenance.gc","annotations":{"readOnlyHint":false,"destructiveHint":true}}
        ]});
        project_operation_groups(&mut catalog).unwrap();
        assert_eq!(catalog["operations"][0]["group"], "catalog");
        assert_eq!(catalog["operations"][1]["group"], "access");
        assert_eq!(catalog["operations"][2]["group"], "operations");
    }

    #[test]
    fn configured_enabled_client_reports_authority_as_unknown() {
        let client = test_client(
            Url::parse("https://depot.invalid/").unwrap(),
            1,
            Duration::from_secs(1),
        );
        assert!(matches!(client.status().authority, DepotAuthority::Unknown));
        assert!(matches!(
            DepotClient::disabled().status().authority,
            DepotAuthority::Unknown
        ));
    }

    #[test]
    fn operation_results_are_wrapped_in_the_labby_compatibility_contract() {
        let response = compatibility_envelope(json!({
            "result": {
                "artifacts": [{
                    "lineage": {
                        "following": false,
                        "upstreamArtifactId": null
                    }
                }],
                "nextCursor": null,
                "total": 1
            }
        }))
        .unwrap();

        assert_eq!(response["schemaVersion"], COMPATIBILITY_SCHEMA_VERSION);
        assert_eq!(response["contractVersion"], 1);
        assert_eq!(response["result"]["total"], 1);
        assert!(response["result"]["nextCursor"].is_null());
        assert!(response["result"]["artifacts"][0]["lineage"]["upstreamArtifactId"].is_null());
    }

    #[test]
    fn non_object_operation_results_fail_closed() {
        assert!(matches!(
            compatibility_envelope(json!([])),
            Err(DepotError::InvalidResponse)
        ));
    }

    #[test]
    fn transport_errors_do_not_disclose_credentials_or_urls() {
        let body = error_body(&DepotError::Unavailable(TransportFailure::Connect)).to_string();
        assert_eq!(body, r#"{"error":"depot_unavailable"}"#);
        assert!(!body.contains("token"));
    }

    #[test]
    fn privileged_upstream_errors_are_redacted_at_the_rust_boundary() {
        let body = error_body(&DepotError::Upstream(
            StatusCode::FORBIDDEN,
            json!({"message":"token secret-token rejected at /private/path"}),
        ));
        assert_eq!(body, json!({"error":"depot_rejected","status":403}));
        assert!(!body.to_string().contains("secret-token"));
        assert!(!body.to_string().contains("/private/path"));
    }

    #[test]
    fn depot_url_requires_an_http_origin() {
        assert!(parse_base_url("https://depot.example.test").is_ok());
        assert!(parse_base_url("https://user:password@depot.example.test").is_err());
        assert!(parse_base_url("https://depot.example.test?token=secret").is_err());
        assert!(parse_base_url("file:///tmp/depot-token").is_err());
        assert!(parse_base_url("not a url").is_err());
    }

    #[tokio::test]
    async fn interactive_queue_wait_is_bounded() {
        let client = test_client(
            Url::parse("http://127.0.0.1:9/").unwrap(),
            1,
            Duration::from_millis(20),
        );
        let _held = client.interactive.acquire().await.unwrap();

        let error = client.session("actor").await.unwrap_err();
        assert!(matches!(error, DepotError::QueueTimeout));
    }

    #[tokio::test]
    async fn connection_failure_retains_sanitized_category() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = test_client(
            Url::parse(&format!("http://{address}/")).unwrap(),
            1,
            Duration::from_secs(2),
        );

        let error = client.session("actor").await.unwrap_err();
        assert!(
            matches!(error, DepotError::Unavailable(TransportFailure::Connect)),
            "expected a connection failure, got {error:?}"
        );
        assert_eq!(error_body(&error), json!({"error":"depot_unavailable"}));
    }
}
