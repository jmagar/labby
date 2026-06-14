pub mod catalog;
pub mod dispatch;
pub mod store;

pub use catalog::ACTIONS;
pub use dispatch::dispatch;

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::store::{
        SnippetSource, create_user_snippet, extract_javascript_block, list_snippets,
        merge_snippet_input, remove_user_snippet, resolve_snippet, validate_snippet_name,
    };

    #[test]
    fn validates_slug_like_snippet_names() {
        assert!(validate_snippet_name("homelab-readonly-pulse").is_ok());
        assert!(validate_snippet_name("repo_context_1").is_ok());

        for name in ["", "../secret", "BadName", "has space", ".hidden"] {
            assert!(
                validate_snippet_name(name).is_err(),
                "{name} should be rejected"
            );
        }
    }

    #[test]
    fn extracts_first_javascript_fence_from_markdown() {
        let source = r#"
# Example

```bash
echo nope
```

```js
async () => ({ ok: true })
```
"#;

        assert_eq!(
            extract_javascript_block(source).unwrap(),
            "async () => ({ ok: true })"
        );
    }

    #[test]
    fn creates_lists_and_resolves_user_snippet() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");
        let builtin_dir = temp.path().join("builtins");
        fs::create_dir_all(&builtin_dir).unwrap();
        fs::write(
            builtin_dir.join("homelab-readonly-pulse.md"),
            "---\nname: homelab-readonly-pulse\ndescription: Homelab pulse\ntags: []\n---\n\n```js\nasync () => ({ ok: true })\n```\n",
        )
        .unwrap();

        create_user_snippet(
            &lab_home,
            "daily-check",
            "```javascript\nasync () => ({ ok: true, source: 'user' })\n```\n",
            Some("Daily check"),
            false,
        )
        .unwrap();

        let snippets = list_snippets(&lab_home, &builtin_dir).unwrap();
        assert_eq!(snippets.len(), 2);
        assert!(
            snippets
                .iter()
                .any(|s| s.name == "daily-check" && s.source == SnippetSource::User)
        );
        assert!(
            snippets
                .iter()
                .any(|s| s.name == "homelab-readonly-pulse" && s.source == SnippetSource::Builtin)
        );

        let resolved = resolve_snippet(&lab_home, &builtin_dir, "daily-check").unwrap();
        assert_eq!(resolved.name, "daily-check");
        assert_eq!(resolved.source, SnippetSource::User);
        assert_eq!(
            extract_javascript_block(&resolved.body).unwrap(),
            "async () => ({ ok: true, source: 'user' })"
        );
    }

    #[test]
    fn refuses_to_remove_builtin_snippet() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");
        let builtin_dir = temp.path().join("builtins");
        fs::create_dir_all(&builtin_dir).unwrap();
        fs::write(
            builtin_dir.join("homelab-readonly-pulse.md"),
            "```js\nasync () => ({ ok: true })\n```\n",
        )
        .unwrap();

        let err = remove_user_snippet(&lab_home, &builtin_dir, "homelab-readonly-pulse")
            .expect_err("built-in snippets are read-only through user remove");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn create_rejects_snippets_without_executable_async_arrow() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");

        let err = create_user_snippet(
            &lab_home,
            "bad",
            "```js\nconsole.log('nope')\n```",
            Some("Bad"),
            false,
        )
        .expect_err("snippet body must be executable");
        assert_eq!(err.kind(), "invalid_param");

        let err = create_user_snippet(
            &lab_home,
            "missing",
            "# no code here",
            Some("Missing"),
            false,
        )
        .expect_err("snippet body must include code");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn create_accepts_raw_async_arrow_body() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");

        let info = create_user_snippet(
            &lab_home,
            "raw",
            "async () => ({ ok: true })",
            Some("Raw snippet"),
            false,
        )
        .expect("raw async arrow snippets are valid");
        let resolved =
            resolve_snippet(&lab_home, &temp.path().join("builtins"), &info.name).unwrap();

        assert_eq!(
            super::store::code_for_snippet(&resolved).unwrap(),
            "async () => ({ ok: true })"
        );
        assert_eq!(resolved.description.as_deref(), Some("Raw snippet"));
    }

    #[test]
    fn frontmatter_name_must_match_snippet_name() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");
        let body = r#"---
name: other
description: Wrong name
tags: []
---

```js
async () => ({ ok: true })
```
"#;

        let err = create_user_snippet(&lab_home, "actual", body, None, false)
            .expect_err("frontmatter name mismatch should fail");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn list_skips_non_executable_markdown_files() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");
        let builtin_dir = temp.path().join("builtins");
        fs::create_dir_all(&builtin_dir).unwrap();
        fs::write(builtin_dir.join("notes.md"), "# just notes\n").unwrap();
        fs::write(
            builtin_dir.join("good.md"),
            "---\nname: good\ndescription: Good snippet\ntags: []\n---\n\n```js\nasync () => ({ ok: true })\n```\n",
        )
        .unwrap();

        let snippets = list_snippets(&lab_home, &builtin_dir).unwrap();

        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].name, "good");
    }

    #[test]
    fn list_keeps_tutorial_sized_builtin_snippets() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");
        let builtin_dir = temp.path().join("builtins");
        fs::create_dir_all(&builtin_dir).unwrap();
        let tutorial = "This tutorial explains the selected tools and schemas.\n".repeat(450);
        fs::write(
            builtin_dir.join("tutorial.md"),
            format!(
                "---\nname: tutorial\ndescription: Tutorial snippet\ntags: [docs]\n---\n\n# Tutorial\n\n{tutorial}\n```js\nasync () => ({{ ok: true }})\n```\n"
            ),
        )
        .unwrap();

        let snippets = list_snippets(&lab_home, &builtin_dir).unwrap();

        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].name, "tutorial");
    }

    #[test]
    fn frontmatter_inputs_merge_defaults_and_validate_params() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");
        let body = r#"---
name: parameterized
description: Parameterized snippet
tags: [demo]
inputs:
  host:
    type: string
    default: dookie
    required: false
  limit:
    type: integer
    default: 5
    required: false
  dry_run:
    type: boolean
    default: false
    required: false
---

```js
async (input) => ({ ok: true, input })
```
"#;

        create_user_snippet(&lab_home, "parameterized", body, None, false).unwrap();
        let resolved =
            resolve_snippet(&lab_home, &temp.path().join("builtins"), "parameterized").unwrap();

        let merged = merge_snippet_input(&resolved, json!({"limit": 3})).unwrap();
        assert_eq!(merged["host"], "dookie");
        assert_eq!(merged["limit"], 3);
        assert_eq!(merged["dry_run"], false);

        let err = merge_snippet_input(&resolved, json!({"limit": "three"})).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");

        let err = merge_snippet_input(&resolved, json!({"extra": true})).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn frontmatter_inputs_enforce_required_params() {
        let temp = tempfile::tempdir().unwrap();
        let lab_home = temp.path().join("lab-home");
        let body = r#"---
name: needs-topic
description: Requires a topic
tags: []
inputs:
  topic:
    type: string
    required: true
---

```js
async (input) => ({ ok: true, topic: input.topic })
```
"#;

        create_user_snippet(&lab_home, "needs-topic", body, None, false).unwrap();
        let resolved =
            resolve_snippet(&lab_home, &temp.path().join("builtins"), "needs-topic").unwrap();

        let err = merge_snippet_input(&resolved, json!({})).unwrap_err();
        assert_eq!(err.kind(), "missing_param");
    }

    #[tokio::test]
    async fn dispatch_lists_snippets_for_mcp_consumers() {
        let value = crate::dispatch::snippets::dispatch("snippets.list", json!({}))
            .await
            .unwrap();

        assert!(value["snippets"].is_array());
    }

    #[tokio::test]
    async fn dispatch_validates_unsaved_snippet_body() {
        let value = crate::dispatch::snippets::dispatch(
            "snippets.validate",
            json!({
                "name": "draft",
                "body": "async () => ({ ok: true })"
            }),
        )
        .await
        .unwrap();

        assert_eq!(value["valid"], true);
        assert_eq!(value["mode"], "body");
        assert_eq!(value["name"], "draft");
    }
}
