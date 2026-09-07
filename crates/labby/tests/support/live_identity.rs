use std::path::Path;
use std::process::Command;

use axum::http::{StatusCode, header};
use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::support::{CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command};

pub(crate) const PROJECT_ID: &str = "bootstrap-default";
pub(crate) const LOADOUT_ID: &str = "production";
pub(crate) const ROUTE_ID: &str = "operator";
pub(crate) const RESOURCE: &str = "https://mcp.example.test/operator";
pub(crate) const PUBLIC_HOST: &str = "lab.example.test";

pub(crate) fn policy(scopes: &[&str]) -> String {
    policy_for_loadout(scopes, LOADOUT_ID)
}

pub(crate) fn policy_for_loadout(scopes: &[&str], loadout_id: &str) -> String {
    let scopes = scopes
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
[[loadouts]]
name = "{loadout_id}"
upstreams = []
services = ["gateway"]

[[protected_mcp_routes]]
name = "{ROUTE_ID}"
enabled = true
public_host = "mcp.example.test"
public_path = "/operator"
scopes = [{scopes}]

[protected_mcp_routes.target]
kind = "gateway_subset"
project_id = "{PROJECT_ID}"
loadout = "{loadout_id}"
"#
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PublicIdentity {
    pub(crate) issuer: String,
    pub(crate) subject: String,
    pub(crate) project_id: String,
    pub(crate) loadout_id: String,
    pub(crate) route_id: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) credential_id: String,
    pub(crate) credential_generation: u64,
    pub(crate) resource: String,
    pub(crate) audience: String,
    pub(crate) expires_at: u64,
}

pub(crate) struct BrowserSession {
    pub(crate) cookie: String,
    pub(crate) csrf: String,
    pub(crate) expires_at: u64,
}

pub(crate) struct LiveIdentity {
    guard: Option<LiveLabbyGuard>,
    owned: tempfile::TempDir,
    client: reqwest::Client,
    proof: String,
    manifest: Value,
    credential: String,
    static_token: String,
    seeded_canary: String,
    retained_evidence: std::path::PathBuf,
    pub(crate) prepare_id: String,
    pub(crate) identity: PublicIdentity,
    pub(crate) session: Option<BrowserSession>,
    journal: Vec<String>,
}

impl LiveIdentity {
    pub(crate) async fn bootstrap(subject: &str) -> Result<Self, String> {
        Self::bootstrap_with_ttl(subject, 300).await
    }

    pub(crate) async fn bootstrap_with_ttl(subject: &str, ttl: u64) -> Result<Self, String> {
        Self::bootstrap_with_policy(subject, ttl, &policy(&["lab:read"])).await
    }

    pub(crate) async fn bootstrap_with_policy(
        subject: &str,
        ttl: u64,
        policy_text: &str,
    ) -> Result<Self, String> {
        Self::bootstrap_with_policy_issuer_and_loadout(
            subject,
            ttl,
            policy_text,
            PUBLIC_HOST,
            LOADOUT_ID,
            &["lab:read"],
        )
        .await
    }

    pub(crate) async fn bootstrap_with_binding(
        subject: &str,
        issuer_host: &str,
        loadout_id: &str,
    ) -> Result<Self, String> {
        Self::bootstrap_with_policy_issuer_and_loadout(
            subject,
            300,
            &policy_for_loadout(&["lab:read"], loadout_id),
            issuer_host,
            loadout_id,
            &["lab:read"],
        )
        .await
    }

    pub(crate) async fn bootstrap_with_scopes(
        subject: &str,
        scopes: &[&str],
    ) -> Result<Self, String> {
        Self::bootstrap_with_policy_issuer_and_loadout(
            subject,
            300,
            &policy(scopes),
            PUBLIC_HOST,
            LOADOUT_ID,
            scopes,
        )
        .await
    }

    async fn bootstrap_with_policy_issuer_and_loadout(
        subject: &str,
        ttl: u64,
        policy_text: &str,
        issuer_host: &str,
        loadout_id: &str,
        prepare_scopes: &[&str],
    ) -> Result<Self, String> {
        let parent = std::env::temp_dir().join("labby-live-e2e");
        std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        let owned = tempfile::Builder::new()
            .prefix("identity-")
            .tempdir_in(parent)
            .map_err(|e| e.to_string())?;
        let root = owned.path().canonicalize().map_err(|e| e.to_string())?;
        let prepared =
            prepare_with_loadout_and_scopes(&root, subject, ttl, loadout_id, prepare_scopes)?;
        let prepare_id = required(&prepared, "prepare_id")?.to_owned();
        let bundle: Value = serde_json::from_slice(
            &std::fs::read(root.join("proof.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let proof = required(&bundle, "proof")?.to_owned();
        let manifest = bundle.get("manifest").cloned().ok_or("missing manifest")?;
        let credential =
            std::fs::read_to_string(root.join("credential.txt")).map_err(|e| e.to_string())?;
        let static_token = format!("identity-static-{}", ulid::Ulid::new());
        let seeded_canary = format!("identity-canary-{}", ulid::Ulid::new());
        let mut guard = LiveLabbyBuilder::new()
            .existing_root(&root)
            .config(policy_text)
            .env("LABBY_MCP_HTTP_TOKEN", &static_token)
            .env("LABBY_WEB_UI_AUTH_DISABLED", "false")
            .env("LABBY_PUBLIC_URL", format!("https://{issuer_host}"))
            .env("LABBY_IDENTITY_CANARY", &seeded_canary)
            .start()
            .await?;
        let retained_evidence = std::env::temp_dir()
            .join("labby-live-e2e-evidence")
            .join(format!("{}.json", guard.identity().run_id));
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| e.to_string())?;
        let base = guard.connection().base_url.clone();
        let consumed = client
            .post(format!("{base}/auth/bootstrap/consume"))
            .header("x-labby-bootstrap-proof", &proof)
            .json(&manifest)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if consumed.status() != StatusCode::OK {
            return Err(format!("consume denied: {}", consumed.status()));
        }
        let introspection = introspect_with(&client, &base, &credential).await?;
        if introspection.0 != StatusCode::OK {
            return Err(format!("introspection denied: {}", introspection.0));
        }
        let body = introspection.1;
        let credential_id = required(&body, "credential_id")?.to_owned();
        let identity = PublicIdentity {
            issuer: required(&manifest, "canonical_issuer")?.into(),
            subject: required(&manifest, "subject")?.into(),
            project_id: required(&body, "project_id")?.into(),
            loadout_id: required(&body, "loadout_id")?.into(),
            route_id: required(&body, "route_id")?.into(),
            scopes: body["scopes"]
                .as_array()
                .ok_or("missing scopes")?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            credential_id: credential_id.clone(),
            credential_generation: required_u64(&body, "credential_generation")?,
            resource: required(&body, "resource")?.into(),
            audience: required(&body, "audience")?.into(),
            expires_at: required_u64(&body, "expires_at")?,
        };
        // The generic process guard stops the daemon before it runs registered
        // revocations. Register the shipped offline recovery path so timeout or
        // panic teardown still revokes this run's exact prepare and removes its
        // secret outputs before the caller-owned installation disappears.
        let mut recovery = installation_command(&root);
        recovery.args([
            "setup",
            "access-bootstrap",
            "recover",
            "--prepare-id",
            &prepare_id,
            "--revoke",
            "--json",
        ]);
        guard.register_credential_session(
            format!("bootstrap:{prepare_id}:{credential_id}"),
            recovery,
            vec![root.join("credential.txt"), root.join("proof.json")],
        )?;
        Ok(Self {
            guard: Some(guard),
            owned,
            client,
            proof,
            manifest,
            credential,
            static_token,
            seeded_canary,
            retained_evidence,
            prepare_id: prepare_id.clone(),
            identity,
            session: None,
            journal: vec![prepare_id, credential_id],
        })
    }

    pub(crate) fn root(&self) -> &Path {
        self.owned.path()
    }

    /// Provision an active same-organization Stash recipient through the
    /// shared live-identity fixture boundary. Tests should not duplicate the
    /// access-store schema or bootstrap-owner identifiers.
    pub(crate) async fn provision_stash_recipient(
        &self,
        principal_id: &str,
        display_name: &str,
    ) -> Result<(), String> {
        labby::testkit::provision_file_stash_recipient(
            self.root().join("labby-home/access.db"),
            self.identity.credential_id.clone(),
            principal_id.to_owned(),
            display_name.to_owned(),
            "static-bearer:primary".to_owned(),
        )
        .await
    }
    pub(crate) fn base(&self) -> &str {
        &self
            .guard
            .as_ref()
            .expect("active identity")
            .connection()
            .base_url
    }
    pub(crate) fn credential_for_request(&self) -> &str {
        &self.credential
    }
    pub(crate) fn static_token_for_request(&self) -> &str {
        &self.static_token
    }
    pub(crate) fn owned_ids(&self) -> &[String] {
        &self.journal
    }
    pub(crate) fn retained_evidence(&self) -> &Path {
        &self.retained_evidence
    }

    pub(crate) fn exact_secret_canaries(&self) -> Vec<String> {
        vec![
            self.credential.clone(),
            self.proof.clone(),
            self.seeded_canary.clone(),
        ]
    }

    pub(crate) async fn exercise_timeout(&mut self) -> Result<(), String> {
        self.guard
            .as_mut()
            .expect("active identity")
            .run_with_timeout(
                std::time::Duration::from_millis(10),
                std::future::pending::<()>(),
            )
            .await
    }

    pub(crate) async fn introspect(&self) -> Result<(StatusCode, Value), String> {
        introspect_with(&self.client, self.base(), &self.credential).await
    }

    pub(crate) async fn repeat_consume(&self) -> Result<StatusCode, String> {
        Ok(self
            .client
            .post(format!("{}/auth/bootstrap/consume", self.base()))
            .header("x-labby-bootstrap-proof", &self.proof)
            .json(&self.manifest)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .status())
    }

    pub(crate) async fn consume_with_manifest(
        &self,
        manifest: Value,
    ) -> Result<StatusCode, String> {
        Ok(self
            .client
            .post(format!("{}/auth/bootstrap/consume", self.base()))
            .header("x-labby-bootstrap-proof", &self.proof)
            .json(&manifest)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .status())
    }

    pub(crate) fn manifest(&self) -> &Value {
        &self.manifest
    }

    pub(crate) async fn protected_mcp_initialize(&self) -> Result<StatusCode, String> {
        self.protected_mcp_initialize_with(&self.credential).await
    }

    pub(crate) async fn protected_mcp_initialize_with(
        &self,
        token: &str,
    ) -> Result<StatusCode, String> {
        Ok(self
            .client
            .post(format!("{}/operator", self.base()))
            .header(header::HOST, "mcp.example.test")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json")
            .bearer_auth(token)
            .json(&json!({
                "jsonrpc":"2.0", "id":1, "method":"initialize",
                "params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"live-identity","version":"1"}}
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .status())
    }

    pub(crate) async fn introspect_token(&self, token: &str) -> Result<StatusCode, String> {
        Ok(self
            .client
            .get(format!("{}/v1/access/credentials/self", self.base()))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .status())
    }

    pub(crate) async fn create_session(&mut self) -> Result<(), String> {
        let response = self
            .client
            .post(format!("{}/auth/local-session", self.base()))
            .header(header::HOST, PUBLIC_HOST)
            .header(header::ORIGIN, format!("https://{PUBLIC_HOST}"))
            .bearer_auth(&self.credential)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status() != StatusCode::CREATED {
            return Err(format!("session denied: {}", response.status()));
        }
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .ok_or("missing session cookie")?
            .to_owned();
        let body: Value = response.json().await.map_err(|e| e.to_string())?;
        let csrf = required(&body, "csrf_token")?.to_owned();
        let expires_at = required_u64(&body, "expires_at")?;
        self.journal
            .push(format!("session:{}", sha256_short(cookie.as_bytes())));
        self.session = Some(BrowserSession {
            cookie,
            csrf,
            expires_at,
        });
        Ok(())
    }

    pub(crate) async fn browser_catalog(&self, csrf: Option<&str>) -> Result<StatusCode, String> {
        Ok(self.browser_catalog_response(csrf).await?.0)
    }

    pub(crate) async fn browser_catalog_response(
        &self,
        csrf: Option<&str>,
    ) -> Result<(StatusCode, Value), String> {
        let session = self.session.as_ref().ok_or("session absent")?;
        let mut request = self
            .client
            .get(format!("{}/v1/catalog", self.base()))
            .header(header::COOKIE, &session.cookie);
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        let body = response.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    pub(crate) async fn bearer_catalog_response(&self) -> Result<(StatusCode, Value), String> {
        let response = self
            .client
            .get(format!("{}/v1/catalog", self.base()))
            .bearer_auth(&self.credential)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        let body = response.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    pub(crate) async fn logout(&self, csrf: &str) -> Result<StatusCode, String> {
        let session = self.session.as_ref().ok_or("session absent")?;
        Ok(self
            .client
            .delete(format!("{}/auth/local-session", self.base()))
            .header(header::HOST, PUBLIC_HOST)
            .header(header::ORIGIN, format!("https://{PUBLIC_HOST}"))
            .header(header::COOKIE, &session.cookie)
            .header("x-csrf-token", csrf)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .status())
    }

    pub(crate) async fn restart(&mut self) -> Result<(), String> {
        self.guard
            .as_mut()
            .expect("active identity")
            .restart()
            .await
    }

    pub(crate) async fn replace_policy_and_restart(&mut self, config: &str) -> Result<(), String> {
        std::fs::write(self.owned.path().join("labby-home/config.toml"), config)
            .map_err(|e| e.to_string())?;
        self.restart().await
    }

    pub(crate) async fn revoke(&self) -> Result<StatusCode, String> {
        self.revoke_id(&self.identity.credential_id).await
    }

    pub(crate) async fn revoke_id(&self, credential_id: &str) -> Result<StatusCode, String> {
        Ok(self
            .client
            .delete(format!(
                "{}/v1/access/credentials/{}",
                self.base(),
                credential_id
            ))
            .bearer_auth(&self.credential)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .status())
    }

    /// Issue a real client-generated equal-or-narrower credential through the
    /// shipped lifecycle endpoint. The returned secret exists only in caller
    /// memory and is registered by the caller's scenario journal.
    pub(crate) async fn issue_narrower_credential(
        &self,
        scopes: &[&str],
    ) -> Result<(String, String), String> {
        Self::issue_narrower_at(
            self.base(),
            &self.credential,
            &self.identity,
            &scopes
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect::<Vec<_>>(),
        )
        .await
    }

    pub(crate) async fn issue_narrower_at(
        base: &str,
        source_credential: &str,
        identity: &PublicIdentity,
        scopes: &[String],
    ) -> Result<(String, String), String> {
        let credential_id = ulid::Ulid::new().to_string();
        let seed = format!("{}:{}", credential_id, ulid::Ulid::new());
        let secret: [u8; 32] = Sha256::digest(seed.as_bytes()).into();
        let wire = format!(
            "{}{}_{}",
            labby_primitives::product_credential::PRODUCT_CREDENTIAL_PREFIX,
            credential_id,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
        );
        let digest = hex::encode(Sha256::digest(wire.as_bytes()));
        let expires_at = identity.expires_at.saturating_sub(1);
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .post(format!("{base}/v1/access/credentials"))
            .bearer_auth(source_credential)
            .json(&json!({
                "credential_id": credential_id,
                "credential_digest_hex": digest,
                "project_id": identity.project_id,
                "route_id": identity.route_id,
                "resource": identity.resource,
                "audience": identity.audience,
                "scopes": scopes,
                "expires_at": expires_at,
                "idempotency_key": format!("parity-issue-{credential_id}")
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status() != StatusCode::CREATED {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("credential issue denied: {status}: {body}"));
        }
        Ok((credential_id, wire))
    }

    pub(crate) async fn cleanup(mut self) -> Result<CleanupResult, String> {
        let session = self.session.as_ref().map(|session| session.cookie.clone());
        let mut failures = Vec::new();
        match self
            .client
            .post(format!("{}/auth/bootstrap/cleanup", self.base()))
            .header("x-labby-bootstrap-proof", &self.proof)
            .json(&json!({"prepare_id": self.prepare_id}))
            .send()
            .await
        {
            Ok(response) if response.status() == StatusCode::OK => {}
            Ok(response) => failures.push(format!("cleanup denied: {}", response.status())),
            Err(error) => failures.push(format!("cleanup request failed: {error}")),
        }
        match self.introspect().await {
            Ok((StatusCode::UNAUTHORIZED, _)) => {}
            Ok((status, _)) => failures.push(format!("credential survived cleanup: {status}")),
            Err(error) => failures.push(format!("credential denial check failed: {error}")),
        }
        if let Some(cookie) = session {
            match self
                .client
                .get(format!("{}/v1/catalog", self.base()))
                .header(header::COOKIE, cookie)
                .send()
                .await
            {
                Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {}
                Ok(response) => failures.push(format!(
                    "browser session survived cleanup: {}",
                    response.status()
                )),
                Err(error) => failures.push(format!("session denial check failed: {error}")),
            }
        }
        for path in ["credential.txt", "proof.json"] {
            if self.owned.path().join(path).exists() {
                failures.push(format!("bootstrap secret output survived cleanup: {path}"));
            }
        }
        let mut guard = self.guard.take().expect("active identity");
        // Successful online cleanup above already proves credential/session
        // denial. Settle only that exact guard, after fallible output-absence
        // verification, rather than repeating a non-idempotent offline revoke.
        if failures.is_empty()
            && let Err(error) = guard.confirm_credential_session_revoked(&format!(
                "bootstrap:{}:{}",
                self.prepare_id, self.identity.credential_id,
            ))
        {
            failures.push(error);
        }
        let result = guard.finish().await;
        failures.extend(result.failures.iter().cloned());
        if let Err(error) = scan_secrets(
            self.owned.path(),
            &[&self.credential, &self.proof, &self.seeded_canary],
        ) {
            failures.push(error);
        }
        if let Err(error) = scan_files(
            std::slice::from_ref(&self.retained_evidence),
            &[&self.credential, &self.proof, &self.seeded_canary],
        ) {
            failures.push(error);
        }
        if failures.is_empty() {
            Ok(result)
        } else {
            Err(failures.join("; "))
        }
    }
}

impl Drop for LiveIdentity {
    fn drop(&mut self) {
        if self.guard.is_none() {
            return;
        }
        // Best-effort online revocation runs while the daemon and installation
        // root are still alive. Explicit `cleanup` performs the full verified path.
        if let Err(error) = synchronous_cleanup_request(self.base(), &self.proof, &self.prepare_id)
        {
            eprintln!("live identity owned cleanup was not acknowledged: {error}");
        }
    }
}

pub(crate) fn prepare(root: &Path, subject: &str, ttl: u64) -> Result<Value, String> {
    prepare_with_loadout(root, subject, ttl, LOADOUT_ID)
}

pub(crate) fn prepare_with_loadout(
    root: &Path,
    subject: &str,
    ttl: u64,
    loadout_id: &str,
) -> Result<Value, String> {
    prepare_with_loadout_and_scopes(root, subject, ttl, loadout_id, &["lab:read"])
}

pub(crate) fn prepare_with_loadout_and_scopes(
    root: &Path,
    subject: &str,
    ttl: u64,
    loadout_id: &str,
    scopes: &[&str],
) -> Result<Value, String> {
    let mut command = installation_command(root);
    command
        .args(["setup", "access-bootstrap", "prepare", "--proof-file"])
        .arg(root.join("proof.json"))
        .arg("--credential-file")
        .arg(root.join("credential.txt"))
        .args([
            "--organization-name",
            "Hermetic Org",
            "--project-name",
            "Hermetic Project",
            "--subject",
            subject,
            "--loadout-id",
            loadout_id,
            "--route-id",
            ROUTE_ID,
            "--resource",
            RESOURCE,
            "--ttl",
            &ttl.to_string(),
        ]);
    for scope in scopes {
        command.args(["--scope", scope]);
    }
    let output = command.arg("--json").output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}

pub(crate) fn recover(root: &Path, prepare_id: &str, revoke: bool) -> Result<Value, String> {
    let mut command = installation_command(root);
    command.args([
        "setup",
        "access-bootstrap",
        "recover",
        "--prepare-id",
        prepare_id,
    ]);
    if revoke {
        command.arg("--revoke");
    }
    let output = command.arg("--json").output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}

fn installation_command(root: &Path) -> Command {
    let home = root.join("home");
    let labby_home = root.join("labby-home");
    for path in [&home, &labby_home, &root.join("tmp"), &root.join("logs")] {
        std::fs::create_dir_all(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let mut command = isolated_command(&home);
    command
        .env("LABBY_HOME", labby_home)
        .env("LABBY_LOG_DIR", root.join("logs"))
        .env("TMPDIR", root.join("tmp"));
    command
}

async fn introspect_with(
    client: &reqwest::Client,
    base: &str,
    token: &str,
) -> Result<(StatusCode, Value), String> {
    let response = client
        .get(format!("{base}/v1/access/credentials/self"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.json().await.unwrap_or(Value::Null);
    Ok((status, body))
}
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))
}
fn required_u64(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing {key}"))
}
fn sha256_short(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    hex::encode(Sha256::digest(bytes))[..16].to_owned()
}
fn scan_secrets(root: &Path, secrets: &[&str]) -> Result<(), String> {
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let file_type = entry.file_type().map_err(|e| e.to_string())?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let bytes = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
                for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
                    if bytes
                        .windows(secret.len())
                        .any(|window| window == secret.as_bytes())
                    {
                        return Err(format!("secret leaked to {}", entry.path().display()));
                    }
                }
            }
        }
    }
    Ok(())
}

fn scan_files(paths: &[std::path::PathBuf], secrets: &[&str]) -> Result<(), String> {
    for path in paths {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
            if bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes())
            {
                return Err(format!("secret leaked to {}", path.display()));
            }
        }
    }
    Ok(())
}

pub(crate) fn scan_retained_evidence(path: &Path, secrets: &[String]) -> Result<(), String> {
    let secrets = secrets.iter().map(String::as_str).collect::<Vec<_>>();
    scan_files(&[path.to_owned()], &secrets)
}

fn synchronous_cleanup_request(base: &str, proof: &str, prepare_id: &str) -> Result<(), String> {
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;
    use std::time::Duration;

    let authority = base
        .strip_prefix("http://")
        .ok_or("cleanup requires local HTTP")?;
    let body =
        serde_json::to_string(&json!({"prepare_id": prepare_id})).map_err(|e| e.to_string())?;
    let mut stream = TcpStream::connect(authority).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    write!(
        stream,
        "POST /auth/bootstrap/cleanup HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nContent-Type: application/json\r\nx-labby-bootstrap-proof: {proof}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    if !response.starts_with(b"HTTP/1.1 200") {
        return Err("online cleanup was not acknowledged".into());
    }
    Ok(())
}
