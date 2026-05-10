use std::io::{self, Write};

use tokio::{
    signal,
    time::{self, Duration},
};

use crate::stats::{SharedStats, StatsSnapshot, format_compact_duration, human_bytes};

const ACTIVE_CONNECTION_LIMIT: usize = 3;
const RECENT_EVENT_LIMIT: usize = 4;
const ENTER_ALTERNATE_SCREEN: &str = "\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J";
const EXIT_ALTERNATE_SCREEN: &str = "\x1b[?25h\x1b[?1049l";

pub fn spawn_live_renderer(stats: SharedStats) {
    print!("{ENTER_ALTERNATE_SCREEN}");
    let _ = io::stdout().flush();

    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            print!("{EXIT_ALTERNATE_SCREEN}");
            let _ = io::stdout().flush();
            std::process::exit(0);
        }
    });

    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;
            let snapshot = stats.snapshot();
            let screen = render_snapshot(&snapshot);
            print!("\x1b[H\x1b[2J{screen}");
            let _ = io::stdout().flush();
        }
    });
}

pub fn render_snapshot(snapshot: &StatsSnapshot) -> String {
    let mut lines = Vec::new();

    lines.push("mNIC-CLI live status".to_string());
    lines.push(format!(
        "Listen: {}    Range split: {}   Elapsed: {}",
        snapshot.listen,
        if snapshot.range_split_enabled {
            "ON "
        } else {
            "OFF"
        },
        format_compact_duration(snapshot.elapsed)
    ));
    lines.push(String::new());

    lines.push("NIC throughput, last 10s average".to_string());
    lines.push(format!(
        "{:<4} {:<15} {:>12} {:>12} {:>7} {:>7} {:>7}",
        "NIC", "IP", "TX/s", "RX/s", "Active", "Opened", "Failed"
    ));
    for nic in &snapshot.nics {
        lines.push(format!(
            "[{}]  {:<15} {:>12} {:>12} {:>7} {:>7} {:>7}",
            nic.index,
            nic.ip,
            format!("{}/s", human_bytes(nic.tx_per_sec)),
            format!("{}/s", human_bytes(nic.rx_per_sec)),
            nic.active_connections,
            nic.opened_connections,
            nic.failed_connections
        ));
    }
    lines.push(String::new());

    lines.push("NIC totals since start".to_string());
    lines.push(format!(
        "{:<4} {:<15} {:>12} {:>12}",
        "NIC", "IP", "TX total", "RX total"
    ));
    for nic in &snapshot.nics {
        lines.push(format!(
            "[{}]  {:<15} {:>12} {:>12}",
            nic.index,
            nic.ip,
            human_bytes(nic.total_tx),
            human_bytes(nic.total_rx)
        ));
    }
    lines.push(String::new());

    lines.push("Active connections".to_string());
    lines.push(format!(
        "{:<5} {:<4} {:<9} {:<38} {:>7} {:>10} {:>10} {:<10}",
        "ID", "NIC", "Kind", "Target", "Age", "TX", "RX", "State"
    ));
    if snapshot.active_connections.is_empty() {
        lines.push("(none)".to_string());
    } else {
        for connection in snapshot
            .active_connections
            .iter()
            .take(ACTIVE_CONNECTION_LIMIT)
        {
            lines.push(format!(
                "{:<5} [{:<1}]  {:<9} {:<38} {:>7} {:>10} {:>10} {:<10}",
                connection.id,
                connection.nic_index,
                truncate(&connection.kind, 9),
                truncate(&connection.target, 38),
                format_compact_duration(connection.age),
                human_bytes(connection.tx),
                human_bytes(connection.rx),
                truncate(&connection.state, 10)
            ));
        }
        if snapshot.active_connections.len() > ACTIVE_CONNECTION_LIMIT {
            lines.push(format!(
                "... {} more active connection(s)",
                snapshot.active_connections.len() - ACTIVE_CONNECTION_LIMIT
            ));
        }
    }
    lines.push(String::new());

    lines.push("Recent events".to_string());
    if snapshot.recent_events.is_empty() {
        lines.push("(none)".to_string());
    } else {
        let start = snapshot
            .recent_events
            .len()
            .saturating_sub(RECENT_EVENT_LIMIT);
        lines.extend(snapshot.recent_events[start..].iter().cloned());
    }

    lines.join("\n")
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }

    let mut truncated = value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>();
    truncated.push('~');
    truncated
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Duration as StdDuration;

    use crate::stats::{ConnectionSnapshot, NicSnapshot, StatsSnapshot};

    use super::*;

    #[test]
    fn renders_range_split_off_status() {
        let snapshot = StatsSnapshot {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            range_split_enabled: false,
            elapsed: StdDuration::from_secs(2),
            nics: [
                NicSnapshot {
                    index: 0,
                    ip: Ipv4Addr::new(192, 168, 1, 10),
                    tx_per_sec: 0,
                    rx_per_sec: 10,
                    total_tx: 0,
                    total_rx: 10,
                    active_connections: 0,
                    opened_connections: 1,
                    failed_connections: 0,
                },
                NicSnapshot {
                    index: 1,
                    ip: Ipv4Addr::new(192, 168, 1, 11),
                    tx_per_sec: 0,
                    rx_per_sec: 0,
                    total_tx: 0,
                    total_rx: 0,
                    active_connections: 0,
                    opened_connections: 0,
                    failed_connections: 0,
                },
            ],
            active_connections: vec![ConnectionSnapshot {
                id: 1,
                nic_index: 0,
                kind: "HTTP GET".to_string(),
                target: "http://raspberrypi.local/file.bin".to_string(),
                age: StdDuration::from_secs(1),
                tx: 0,
                rx: 10,
                state: "full".to_string(),
            }],
            recent_events: vec!["event".to_string()],
        };

        let screen = render_snapshot(&snapshot);
        assert!(screen.contains("Range split: OFF"));
        assert!(screen.contains("Recent events"));
    }
}
