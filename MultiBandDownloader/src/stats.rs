use std::{
    collections::{BTreeMap, VecDeque},
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{cli::RuntimeOptions, config::Config};

const RECENT_EVENT_LIMIT: usize = 10;
const THROUGHPUT_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct SharedStats {
    inner: Arc<Mutex<Stats>>,
}

#[derive(Debug)]
struct Stats {
    started_at: Instant,
    listen: SocketAddr,
    range_split_enabled: bool,
    nics: [NicStats; 2],
    traffic_samples: VecDeque<TrafficSample>,
    active_connections: BTreeMap<u64, ConnectionStats>,
    recent_events: VecDeque<String>,
}

#[derive(Debug, Clone)]
struct NicStats {
    ip: Ipv4Addr,
    total_tx: u64,
    total_rx: u64,
    opened_connections: u64,
    failed_connections: u64,
}

#[derive(Debug, Clone)]
struct TrafficSample {
    sampled_at: Instant,
    nics: [NicTotals; 2],
}

#[derive(Debug, Clone, Copy)]
struct NicTotals {
    tx: u64,
    rx: u64,
}

#[derive(Debug, Clone)]
struct ConnectionStats {
    id: u64,
    nic_index: usize,
    kind: String,
    target: String,
    started_at: Instant,
    tx: u64,
    rx: u64,
    state: String,
}

#[derive(Debug, Clone)]
pub struct StatsSnapshot {
    pub listen: SocketAddr,
    pub range_split_enabled: bool,
    pub elapsed: Duration,
    pub nics: [NicSnapshot; 2],
    pub active_connections: Vec<ConnectionSnapshot>,
    pub recent_events: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NicSnapshot {
    pub index: usize,
    pub ip: Ipv4Addr,
    pub tx_per_sec: u64,
    pub rx_per_sec: u64,
    pub total_tx: u64,
    pub total_rx: u64,
    pub active_connections: usize,
    pub opened_connections: u64,
    pub failed_connections: u64,
}

#[derive(Debug, Clone)]
pub struct ConnectionSnapshot {
    pub id: u64,
    pub nic_index: usize,
    pub kind: String,
    pub target: String,
    pub age: Duration,
    pub tx: u64,
    pub rx: u64,
    pub state: String,
}

impl SharedStats {
    pub fn new(config: &Config, options: RuntimeOptions) -> Self {
        let now = Instant::now();
        let nics = [
            NicStats::new(config.nics[0].ip),
            NicStats::new(config.nics[1].ip),
        ];
        let mut traffic_samples = VecDeque::new();
        traffic_samples.push_back(TrafficSample::from_nics(now, &nics));

        Self {
            inner: Arc::new(Mutex::new(Stats {
                started_at: now,
                listen: config.proxy.listen,
                range_split_enabled: options.range_split_enabled,
                nics,
                traffic_samples,
                active_connections: BTreeMap::new(),
                recent_events: VecDeque::new(),
            })),
        }
    }

    pub fn record_inbound(&self, id: u64, peer: SocketAddr, nic_index: usize) {
        let mut stats = self.lock();
        let nic_ip = stats.nics[nic_index].ip;
        stats.nics[nic_index].opened_connections += 1;
        stats.active_connections.insert(
            id,
            ConnectionStats::new(id, nic_index, "TCP", peer.to_string(), "accepted"),
        );
        stats.push_event(format!(
            "inbound #{id} from {peer} assigned NIC[{nic_index}] {nic_ip}"
        ));
    }

    pub fn start_connection(
        &self,
        id: u64,
        nic_index: usize,
        kind: impl Into<String>,
        target: impl Into<String>,
        state: impl Into<String>,
    ) {
        let mut stats = self.lock();
        let kind = kind.into();
        let target = target.into();
        let state = state.into();
        let entry = stats
            .active_connections
            .entry(id)
            .or_insert_with(|| ConnectionStats::new(id, nic_index, &kind, &target, &state));
        entry.nic_index = nic_index;
        entry.kind = kind.clone();
        entry.target = target.clone();
        entry.state = state.clone();
        stats.push_event(format!(
            "{} #{id} {target} {state} via NIC[{nic_index}]",
            kind
        ));
    }

    pub fn set_state(&self, id: u64, state: impl Into<String>) {
        let mut stats = self.lock();
        if let Some(connection) = stats.active_connections.get_mut(&id) {
            connection.state = state.into();
        }
    }

    pub fn record_event(&self, event: impl Into<String>) {
        self.lock().push_event(event.into());
    }

    pub fn add_tx(&self, id: u64, nic_index: usize, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let mut stats = self.lock();
        stats.nics[nic_index].total_tx += bytes;
        if let Some(connection) = stats.active_connections.get_mut(&id) {
            connection.tx += bytes;
        }
    }

    pub fn add_rx(&self, id: u64, nic_index: usize, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let mut stats = self.lock();
        stats.nics[nic_index].total_rx += bytes;
        if let Some(connection) = stats.active_connections.get_mut(&id) {
            connection.rx += bytes;
        }
    }

    pub fn finish_connection(&self, id: u64, state: impl AsRef<str>) {
        let mut stats = self.lock();
        let Some(mut connection) = stats.active_connections.remove(&id) else {
            return;
        };
        connection.state = state.as_ref().to_string();
        stats.push_event(format!(
            "{} #{} {} after {} tx {} rx {}",
            connection.kind,
            connection.id,
            connection.state,
            format_compact_duration(connection.started_at.elapsed()),
            human_bytes(connection.tx),
            human_bytes(connection.rx)
        ));
    }

    pub fn finish_pending_tcp(&self, id: u64) {
        let mut stats = self.lock();
        let Some(connection) = stats.active_connections.get(&id) else {
            return;
        };
        if connection.kind != "TCP" {
            return;
        }

        stats.active_connections.remove(&id);
    }

    pub fn fail_connection(&self, id: u64, error: impl AsRef<str>) {
        let mut stats = self.lock();
        let error = error.as_ref();
        if let Some(connection) = stats.active_connections.remove(&id) {
            stats.nics[connection.nic_index].failed_connections += 1;
            stats.push_event(format!(
                "{} #{} failed after {} tx {} rx {}: {}",
                connection.kind,
                connection.id,
                format_compact_duration(connection.started_at.elapsed()),
                human_bytes(connection.tx),
                human_bytes(connection.rx),
                error
            ));
        } else {
            stats.push_event(format!("connection #{id} failed: {error}"));
        }
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        let mut stats = self.lock();
        stats.snapshot()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Stats> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Stats {
    fn push_event(&mut self, event: String) {
        let elapsed = format_compact_duration(self.started_at.elapsed());
        self.recent_events.push_back(format!("+{elapsed}  {event}"));
        while self.recent_events.len() > RECENT_EVENT_LIMIT {
            self.recent_events.pop_front();
        }
    }

    fn snapshot(&mut self) -> StatsSnapshot {
        let now = Instant::now();
        self.traffic_samples
            .push_back(TrafficSample::from_nics(now, &self.nics));
        while self.traffic_samples.len() > 1
            && now.duration_since(
                self.traffic_samples
                    .front()
                    .expect("sample exists")
                    .sampled_at,
            ) > THROUGHPUT_WINDOW
        {
            self.traffic_samples.pop_front();
        }

        let baseline = self.traffic_samples.front().expect("sample exists").clone();
        let sample_duration = now
            .duration_since(baseline.sampled_at)
            .max(Duration::from_secs(1));
        let sample_secs = sample_duration.as_secs_f64();
        let active_by_nic = self.active_by_nic();
        let nics = std::array::from_fn(|index| {
            let nic = &self.nics[index];
            let tx_delta = nic.total_tx.saturating_sub(baseline.nics[index].tx);
            let rx_delta = nic.total_rx.saturating_sub(baseline.nics[index].rx);
            NicSnapshot {
                index,
                ip: nic.ip,
                tx_per_sec: (tx_delta as f64 / sample_secs) as u64,
                rx_per_sec: (rx_delta as f64 / sample_secs) as u64,
                total_tx: nic.total_tx,
                total_rx: nic.total_rx,
                active_connections: active_by_nic[index],
                opened_connections: nic.opened_connections,
                failed_connections: nic.failed_connections,
            }
        });

        StatsSnapshot {
            listen: self.listen,
            range_split_enabled: self.range_split_enabled,
            elapsed: self.started_at.elapsed(),
            nics,
            active_connections: self
                .active_connections
                .values()
                .take(10)
                .map(|connection| connection.snapshot())
                .collect(),
            recent_events: self.recent_events.iter().cloned().collect(),
        }
    }

    fn active_by_nic(&self) -> [usize; 2] {
        let mut active = [0, 0];
        for connection in self.active_connections.values() {
            if connection.nic_index < active.len() {
                active[connection.nic_index] += 1;
            }
        }
        active
    }
}

impl NicStats {
    fn new(ip: Ipv4Addr) -> Self {
        Self {
            ip,
            total_tx: 0,
            total_rx: 0,
            opened_connections: 0,
            failed_connections: 0,
        }
    }
}

impl TrafficSample {
    fn from_nics(sampled_at: Instant, nics: &[NicStats; 2]) -> Self {
        Self {
            sampled_at,
            nics: std::array::from_fn(|index| NicTotals {
                tx: nics[index].total_tx,
                rx: nics[index].total_rx,
            }),
        }
    }
}

impl ConnectionStats {
    fn new(
        id: u64,
        nic_index: usize,
        kind: impl Into<String>,
        target: impl Into<String>,
        state: impl Into<String>,
    ) -> Self {
        Self {
            id,
            nic_index,
            kind: kind.into(),
            target: target.into(),
            started_at: Instant::now(),
            tx: 0,
            rx: 0,
            state: state.into(),
        }
    }

    fn snapshot(&self) -> ConnectionSnapshot {
        ConnectionSnapshot {
            id: self.id,
            nic_index: self.nic_index,
            kind: self.kind.clone(),
            target: self.target.clone(),
            age: self.started_at.elapsed(),
            tx: self.tx,
            rx: self.rx,
            state: self.state.clone(),
        }
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;

    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn format_compact_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;

    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use crate::config::{Config, NicConfig, ProxyConfig};

    use super::*;

    fn test_config() -> Config {
        Config {
            nics: vec![
                NicConfig {
                    ip: Ipv4Addr::new(192, 168, 1, 10),
                },
                NicConfig {
                    ip: Ipv4Addr::new(192, 168, 1, 11),
                },
            ],
            proxy: ProxyConfig {
                listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            },
        }
    }

    #[test]
    fn accumulates_tx_and_rx() {
        let stats = SharedStats::new(&test_config(), RuntimeOptions::default());
        stats.record_inbound(1, SocketAddr::from(([127, 0, 0, 1], 5000)), 0);
        stats.add_tx(1, 0, 10);
        stats.add_rx(1, 0, 20);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.nics[0].total_tx, 10);
        assert_eq!(snapshot.nics[0].total_rx, 20);
    }

    #[test]
    fn failed_connections_keep_counted_bytes() {
        let stats = SharedStats::new(&test_config(), RuntimeOptions::default());
        stats.record_inbound(1, SocketAddr::from(([127, 0, 0, 1], 5000)), 1);
        stats.add_rx(1, 1, 99);
        stats.fail_connection(1, "boom");

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.nics[1].total_rx, 99);
        assert_eq!(snapshot.nics[1].failed_connections, 1);
        assert!(snapshot.active_connections.is_empty());
    }

    #[test]
    fn closes_only_pending_tcp_connections() {
        let stats = SharedStats::new(&test_config(), RuntimeOptions::default());
        stats.record_inbound(1, SocketAddr::from(([127, 0, 0, 1], 5000)), 0);
        stats.record_inbound(2, SocketAddr::from(([127, 0, 0, 1], 5001)), 1);
        stats.start_connection(2, 1, "CONNECT", "example.com:443", "open");

        stats.finish_pending_tcp(1);
        stats.finish_pending_tcp(2);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.active_connections.len(), 1);
        assert_eq!(snapshot.active_connections[0].id, 2);
    }
}
