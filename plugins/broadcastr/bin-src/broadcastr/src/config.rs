use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub session_id: String,
    pub repo: PathBuf,
    pub per_repo_bus: PathBuf,
    pub global_bus: Option<PathBuf>,
    pub want_global: bool,
    pub mute: Vec<String>,
    pub disabled: bool,
    pub bus_max_bytes: u64,
    pub bus_retain: u32,
    pub apprise_enabled: bool,
    pub apprise_tag: String,
}

impl Config {
    pub fn from_env() -> Self {
        let repo = std::env::var("CLAUDE_PROJECT_DIR")
            .or_else(|_| std::env::var("PWD"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));

        let per_repo_bus = repo.join(".broadcastr/events.jsonl");

        let global_home = std::env::var("BROADCASTR_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join(".claude/broadcastr"));
        let global_bus = Some(global_home.join("events.jsonl"));

        let want_global = std::env::var("BROADCASTR_GLOBAL_FEED").as_deref() != Ok("0");

        let mute = std::env::var("BROADCASTR_MUTE")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        let disabled = std::env::var("BROADCASTR_DISABLED").as_deref() == Ok("1");

        let bus_max_bytes = std::env::var("BROADCASTR_BUS_MAX_BYTES")
            .ok().and_then(|s| s.parse().ok())
            .unwrap_or(5 * 1024 * 1024);

        let bus_retain = std::env::var("BROADCASTR_BUS_RETAIN")
            .ok().and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(3).max(1);

        let apprise_enabled = std::env::var("CLAUDE_PLUGIN_OPTION_APPRISE_ENABLED")
            .unwrap_or_else(|_| "true".into()) == "true";
        let apprise_tag = std::env::var("CLAUDE_PLUGIN_OPTION_APPRISE_TAG")
            .unwrap_or_else(|_| "broadcastr".into());

        let session_id = std::env::var("CLAUDE_SESSION_ID").unwrap_or_default();

        Self {
            session_id,
            repo,
            per_repo_bus,
            global_bus,
            want_global,
            mute,
            disabled,
            bus_max_bytes,
            bus_retain,
            apprise_enabled,
            apprise_tag,
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
