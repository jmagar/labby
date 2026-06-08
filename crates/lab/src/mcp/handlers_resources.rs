//! Resource handler bodies (`list_resources`, `read_resource`).
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.3`) as inherent
//! `impl LabMcpServer` methods. The `ServerHandler` trait impl in
//! `server.rs` keeps one-line delegators.
//!
//! `read_resource_impl` keeps the prefix-dispatch skeleton + the local
//! `lab://catalog` / `lab://<svc>/actions` branch; the three proxy
//! branches live in `resource_proxy.rs` and are reached via the same
//! guard ordering as the original (gateway → upstream → subject-scoped).
//!
//! No behavior change — relocation only.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    AnnotateAble, ListResourcesResult, LoggingLevel, Meta, PaginatedRequestParams, RawResource,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents,
};
use rmcp::service::RequestContext;
use serde_json::{Value, json};

use crate::mcp::context::{
    auth_context_from_extensions, code_mode_search_scope_allowed,
    oauth_upstream_subject_for_request,
};
use crate::mcp::logging::DispatchLogOutcome;
use crate::mcp::server::LabMcpServer;

pub(crate) const CODE_MODE_APP_MIME: &str = "text/html;profile=mcp-app";
pub(crate) const CODE_MODE_SEARCH_APP_URI: &str = "ui://lab/code-mode/search";
pub(crate) const CODE_MODE_EXECUTE_APP_URI: &str = "ui://lab/code-mode/execute";
pub(crate) const CODE_MODE_HISTORY_APP_URI: &str = "ui://lab/code-mode/history";

const CODE_MODE_APP_FALLBACK_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Lab Code Mode Inspector</title>
<style>
:root{color-scheme:dark;background:#07131c;color:#e6f4fb;font-family:Inter,system-ui,sans-serif}
*{box-sizing:border-box}
body{margin:0;padding:14px;background:#07131c;color:#e6f4fb}
main{display:grid;gap:12px}
header{display:flex;align-items:flex-start;justify-content:space-between;gap:12px;border-bottom:1px solid #1d3d4e;padding-bottom:10px}
h1{font-size:16px;line-height:1.25;margin:0;font-weight:700}
.sub{color:#a7bcc9;font-size:12px;margin-top:3px}
.badge{border:1px solid #1d3d4e;border-radius:999px;padding:3px 8px;color:#72c8f5;font-size:11px;white-space:nowrap}
.grid{display:grid;gap:8px}
.card{border:1px solid #1d3d4e;border-radius:8px;background:#102330;padding:10px}
.row{display:flex;gap:8px;align-items:center;justify-content:space-between}
.name{font-family:"JetBrains Mono",ui-monospace,monospace;font-size:12px;color:#e6f4fb;overflow-wrap:anywhere}
.meta{font-size:11px;color:#a7bcc9}
.ok{color:#7dd3c7}.err{color:#c78490}.info{color:#72c8f5}
details{margin-top:8px}
summary{cursor:pointer;color:#67cbfa;font-size:12px}
pre{margin:8px 0 0;white-space:pre-wrap;overflow-wrap:anywhere;border:1px solid #1d3d4e;border-radius:6px;background:#07131c;padding:8px;color:#e6f4fb;font-size:11px}
</style>
</head>
<body>
<main>
<header>
<div><h1>Code Mode Inspector</h1><div class="sub" id="summary">Waiting for a tool result</div></div>
<div class="badge" id="state">read only</div>
</header>
<section class="grid" id="content"></section>
</main>
<script>window.__LAB_CODE_MODE_INITIAL_TRACE__ = null;</script>
<script>
const content=document.getElementById("content");
const summary=document.getElementById("summary");
const state=document.getElementById("state");
function esc(value){return String(value??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));}
function json(value){try{return JSON.stringify(value,null,2)}catch{return String(value)}}
function shape(s){if(!s)return"";return [s.type,s.key_count&&`${s.key_count} keys`,s.length&&`${s.length} items`].filter(Boolean).join(" / ")}
function card(body){return `<article class="card">${body}</article>`}
function statusLabel(item){return item.ok?"ok":(item.error_kind||"error")}
function callRow(call){
  const cls=call.ok?"ok":"err";
  const params=call.params?`<details><summary>Params</summary><pre>${esc(json(call.params))}</pre></details>`:"";
  return card(`<div class="row"><div class="name">${esc(call.upstream||"upstream")} / ${esc(call.tool||"tool")}</div><div class="${cls}">${esc(statusLabel(call))}</div></div><div class="meta">${esc(call.elapsed_ms??0)} ms ${call.error_kind?` / ${esc(call.error_kind)}`:""}</div>${call.result_shape?`<div class="meta">${esc(shape(call.result_shape))}</div>`:""}${params}`);
}
function matchRow(match){return card(`<div class="row"><div class="name">${esc(match.id||match.name||"match")}</div><div class="info">${esc(match.upstream||"")}</div></div><div class="meta">${esc(match.description||"")}</div>`)}
function historyRow(entry){
  const count=Array.isArray(entry.calls)?entry.calls.length:0;
  return card(`<div class="row"><div class="name">${esc(entry.kind||"entry")}</div><div class="${entry.ok?"ok":"err"}">${esc(statusLabel(entry))}</div></div><div class="meta">${esc(entry.elapsed_ms??0)} ms / ${count} calls${entry.match_count!==undefined?` / ${esc(entry.match_count)} matches`:""}</div>`);
}
function render(trace){
  const t=trace&&trace.structuredContent?trace.structuredContent:trace;
  if(!t||!t.kind){content.innerHTML=card('<div class="meta">Run Code Mode search or execute to populate the inspector.</div>');return;}
  if(t.kind==="code_mode_execute_trace"){summary.textContent=`${t.call_count||0} broker calls captured`;content.innerHTML=(t.calls||[]).map(callRow).join("")||card('<div class="meta">No broker calls were made.</div>');return;}
  if(t.kind==="code_mode_search_trace"){summary.textContent=`${t.match_count||0} catalog matches`;content.innerHTML=(t.matches||[]).map(matchRow).join("")||card('<div class="meta">No matches.</div>');return;}
  if(t.kind==="code_mode_history"){summary.textContent=`${(t.entries||[]).length} bounded history entries`;content.innerHTML=(t.entries||[]).map(historyRow).join("")||card('<div class="meta">History is empty.</div>');return;}
  content.innerHTML=card(`<pre>${esc(json(t))}</pre>`);
}
render(window.__LAB_CODE_MODE_INITIAL_TRACE__);
const Host=window.ExtApps&&window.ExtApps.App;
if(Host){
  try{
    const app=new Host();
    app.ontoolresult=(result)=>render(result&&result.structuredContent?result.structuredContent:result);
    Promise.resolve(app.connect&&app.connect()).then(()=>{state.textContent="connected"}).catch(()=>{state.textContent="read only"});
  }catch{state.textContent="read only";}
}
</script>
</body>
</html>"#;

impl LabMcpServer {
    pub(crate) async fn list_resources_impl(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_resources",
            subject,
            "dispatch start"
        );
        let mut resources = vec![
            RawResource::new("lab://catalog", "catalog")
                .with_description("Full discovery document for all services")
                .with_mime_type("application/json")
                .no_annotation(),
        ];
        if code_mode_app_resources_visible(
            self.code_mode_visibility().await.exposes_synthetic_tools(),
        ) {
            resources.extend(code_mode_app_resources());
        }

        for svc in self.registry.services() {
            if self.service_visible_on_mcp(svc.name).await {
                let uri = format!("lab://{}/actions", svc.name);
                let name = format!("{}/actions", svc.name);
                resources.push(
                    RawResource::new(uri, name)
                        .with_description(format!("Action list for {}", svc.name))
                        .with_mime_type("application/json")
                        .no_annotation(),
                );
            }
        }

        if let Some(pool) = self.current_upstream_pool().await {
            resources.extend(pool.gateway_synthetic_resources().await);
            resources.extend(pool.list_upstream_resources().await);
            let auth = auth_context_from_extensions(&context.extensions);
            if let Some(oauth_subject) =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                let configs = self.oauth_upstream_configs().await;
                resources.extend(
                    pool.subject_scoped_resources(&configs, oauth_subject.as_ref())
                        .await,
                );
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_resources",
            subject,
            elapsed_ms,
            "resource list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_resources",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ListResourcesResult::with_all_items(resources))
    }

    pub(crate) async fn read_resource_impl(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let uri = &request.uri;
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            resource_uri = crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri),
            "dispatch start"
        );

        // Branch 0: MCP Apps UI resources. This must precede all lab://
        // fallbacks so ui:// has its own exact lookup semantics.
        if uri.starts_with("ui://") {
            return self
                .read_code_mode_app_resource_impl(uri, &subject, start, &context)
                .await;
        }

        // Branch 1: gateway-synthetic resources.
        if uri.starts_with("lab://gateway/") {
            return self
                .read_gateway_resource_impl(uri, &subject, start, &context)
                .await;
        }

        // Branch 2: raw upstream resource proxy.
        if let Some(pool) = self.current_upstream_pool().await
            && uri.starts_with("lab://upstream/")
        {
            return self
                .read_upstream_resource_impl(&pool, uri, &subject, start, &context)
                .await;
        }

        // Branch 3: subject-scoped upstream resource proxy.
        let auth = auth_context_from_extensions(&context.extensions);
        if let Some(oauth_subject) =
            oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            && let Some(pool) = self.current_upstream_pool().await
            && let Some(upstream_name) = uri
                .strip_prefix("lab://upstream/")
                .and_then(|rest| rest.split('/').next())
            && let Some(config) = self.oauth_upstream_config(upstream_name).await
        {
            return self
                .read_subject_scoped_resource_impl(
                    &pool,
                    &config,
                    oauth_subject.as_ref(),
                    uri,
                    &subject,
                    start,
                    &context,
                )
                .await;
        }

        // Local branch: lab://catalog + lab://<svc>/actions.
        let json = if uri == "lab://catalog" {
            self.catalog_json().await
        } else if let Some(service) = uri
            .strip_prefix("lab://")
            .and_then(|value| value.strip_suffix("/actions"))
        {
            self.service_actions_json(service).await
        } else {
            return Err(ErrorData::resource_not_found(
                format!("unknown resource: {uri}"),
                None,
            ));
        };

        match json {
            Ok(value) => {
                let text = match serde_json::to_string_pretty(&value) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!(
                            surface = "mcp",
                            service = "labby",
                            action = "read_resource",
                            subject,
                            error = %e,
                            "failed to serialize resource"
                        );
                        return Err(ErrorData::internal_error(
                            format!("failed to serialize resource: {e}"),
                            None,
                        ));
                    }
                };
                let elapsed_ms = start.elapsed().as_millis();
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    elapsed_ms,
                    "resource read ok"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Success,
                )
                .await;
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(text, uri.clone()).with_mime_type("application/json"),
                ]))
            }
            Err(e) => {
                let elapsed_ms = start.elapsed().as_millis();
                tracing::error!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    elapsed_ms,
                    kind = "internal_error",
                    "resource read failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Error,
                        kind: "internal_error",
                    },
                )
                .await;
                Err(ErrorData::internal_error(e.to_string(), None))
            }
        }
    }

    async fn read_code_mode_app_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.code_mode_visibility().await.exposes_synthetic_tools() {
            return Err(ErrorData::resource_not_found(
                format!("unknown UI resource: {uri}"),
                None,
            ));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !code_mode_search_scope_allowed(auth) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = uri,
                "code mode app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(ErrorData::invalid_params(
                "Code Mode app resources require one of scopes: lab:read, lab, lab:admin",
                Some(json!({
                    "kind": "forbidden",
                    "required_scopes": ["lab:read", "lab", "lab:admin"],
                })),
            ));
        }
        let history = if uri == CODE_MODE_HISTORY_APP_URI {
            match &self.gateway_manager {
                Some(manager) => Some(json!({
                    "kind": "code_mode_history",
                    "entries": manager.code_mode_history_snapshot().await,
                })),
                None => Some(json!({ "kind": "code_mode_history", "entries": [] })),
            }
        } else {
            None
        };
        let html = code_mode_app_html(uri, history.as_ref())
            .map_err(|message| ErrorData::resource_not_found(message, None))?;
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
            "code mode app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(CODE_MODE_APP_MIME)
                .with_meta(code_mode_app_resource_meta(uri)),
        ]))
    }
}

fn code_mode_app_html(uri: &str, history: Option<&Value>) -> Result<String, String> {
    if !matches!(
        uri,
        CODE_MODE_SEARCH_APP_URI | CODE_MODE_EXECUTE_APP_URI | CODE_MODE_HISTORY_APP_URI
    ) {
        return Err(format!("unknown UI resource: {uri}"));
    }

    let mut html = CODE_MODE_APP_FALLBACK_HTML.to_string();
    if let Some(snapshot) = history {
        let injected = format!(
            "window.__LAB_CODE_MODE_INITIAL_TRACE__ = {};",
            snapshot.to_string().replace('<', "\\u003c")
        );
        html = html.replace("window.__LAB_CODE_MODE_INITIAL_TRACE__ = null;", &injected);
    }
    Ok(html)
}

fn code_mode_app_resource(uri: &str, name: &str) -> rmcp::model::Resource {
    RawResource::new(uri.to_string(), name.to_string())
        .with_description("Read-only MCP App for Code Mode call traces")
        .with_mime_type(CODE_MODE_APP_MIME)
        .with_meta(code_mode_app_resource_meta(uri))
        .no_annotation()
}

fn code_mode_app_resources_visible(exposes_synthetic_tools: bool) -> bool {
    exposes_synthetic_tools
}

fn code_mode_app_resources() -> [rmcp::model::Resource; 3] {
    [
        code_mode_app_resource(CODE_MODE_SEARCH_APP_URI, "code-mode/search"),
        code_mode_app_resource(CODE_MODE_EXECUTE_APP_URI, "code-mode/execute"),
        code_mode_app_resource(CODE_MODE_HISTORY_APP_URI, "code-mode/history"),
    ]
}

pub(crate) fn code_mode_app_resource_meta(uri: &str) -> Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "ui".to_string(),
        json!({
            "resourceUri": uri,
            "mimeTypes": [CODE_MODE_APP_MIME],
        }),
    );
    meta.insert(
        "csp".to_string(),
        json!({
            "connectDomains": [],
            "resourceDomains": [],
            "frameDomains": [],
        }),
    );
    meta.insert("prefersBorder".to_string(), Value::Bool(false));
    Meta(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_mode_app_resource_meta_uses_mcp_app_mime_and_csp() {
        let meta = code_mode_app_resource_meta(CODE_MODE_SEARCH_APP_URI);
        assert_eq!(
            meta.0["ui"]["resourceUri"].as_str(),
            Some(CODE_MODE_SEARCH_APP_URI)
        );
        assert_eq!(
            meta.0["ui"]["mimeTypes"][0].as_str(),
            Some(CODE_MODE_APP_MIME)
        );
        assert_eq!(
            meta.0["csp"]["connectDomains"]
                .as_array()
                .expect("connect domains")
                .len(),
            0
        );
    }

    #[test]
    fn code_mode_app_html_accepts_known_ui_resources_and_rejects_unknown() {
        let html = code_mode_app_html(CODE_MODE_EXECUTE_APP_URI, None).expect("known resource");
        assert!(html.contains("Lab Code Mode Inspector"));

        let err = code_mode_app_html("ui://lab/code-mode/nope", None).expect_err("unknown");
        assert!(err.contains("unknown UI resource"));
    }

    #[test]
    fn code_mode_app_resources_follow_synthetic_tool_visibility() {
        assert!(
            code_mode_app_resources_visible(true),
            "Code Mode app resources should be listed with synthetic search/execute tools"
        );
        assert!(
            !code_mode_app_resources_visible(false),
            "Code Mode app resources should not be listed when synthetic tools are disabled"
        );
        let resources = code_mode_app_resources();
        let uris = resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            uris,
            vec![
                CODE_MODE_SEARCH_APP_URI,
                CODE_MODE_EXECUTE_APP_URI,
                CODE_MODE_HISTORY_APP_URI
            ]
        );
    }

    #[test]
    fn code_mode_history_html_injects_escaped_snapshot() {
        let html = code_mode_app_html(
            CODE_MODE_HISTORY_APP_URI,
            Some(&json!({
                "kind": "code_mode_history",
                "entries": [{"seq": 1, "kind": "execute", "ok": true, "elapsed_ms": 1, "calls": [{"params": {"note": "</script>"}}]}],
            })),
        )
        .expect("history resource");

        assert!(html.contains("code_mode_history"));
        assert!(!html.contains("</script>\""));
        assert!(html.contains("\\u003c/script>"));
    }

    #[test]
    fn code_mode_app_html_uses_current_trace_field_names() {
        let html = code_mode_app_html(
            CODE_MODE_EXECUTE_APP_URI,
            Some(&json!({
                "kind": "code_mode_execute_trace",
                "call_count": 1,
                "calls": [{
                    "id": "github::search_issues",
                    "upstream": "github",
                    "tool": "search_issues",
                    "ok": true,
                    "elapsed_ms": 12,
                    "result_shape": {"type": "array", "length": 3},
                }],
            })),
        )
        .expect("execute resource");

        assert!(html.contains("statusLabel"));
        assert!(html.contains("call.ok"));
        assert!(html.contains("s.length"));
        assert!(
            !html.contains("call.status"),
            "inline app must use the emitted ok boolean, not stale status fields"
        );
        assert!(
            !html.contains("array_len"),
            "inline app must use result_shape.length"
        );
    }
}
