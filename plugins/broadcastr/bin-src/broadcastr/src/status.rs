use std::io;

use crate::config::Config;

pub fn run(config: &Config) -> io::Result<()> {
    println!("broadcastr status");

    let mut buses = vec![config.per_repo_bus.clone()];
    if let Some(g) = &config.global_bus {
        buses.push(g.clone());
    }

    for bus in &buses {
        if bus.exists() {
            let size = std::fs::metadata(bus).map(|m| m.len()).unwrap_or(0);
            let events = std::fs::read_to_string(bus)
                .map(|s| s.lines().count())
                .unwrap_or(0);
            println!("  {}  {} bytes  {} events", bus.display(), size, events);
        } else {
            println!("  {}  (absent)", bus.display());
        }
    }

    println!("  disabled={}  global_feed={}  mute={}",
        config.disabled as u8,
        config.want_global as u8,
        config.mute.join(","),
    );
    Ok(())
}
