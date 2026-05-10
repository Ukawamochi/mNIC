use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::{cli::RuntimeOptions, config::Config, handler, http, output, stats::SharedStats};

#[derive(Clone)]
pub struct ProxyState {
    pub config: Arc<Config>,
    pub clients: Arc<Vec<reqwest::Client>>,
    pub options: RuntimeOptions,
    pub stats: SharedStats,
    next_nic_index: Arc<AtomicUsize>,
    next_connection_id: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectionContext {
    pub id: u64,
    pub nic_index: usize,
    pub peer: SocketAddr,
}

impl ProxyState {
    fn next_connection(&self, peer: SocketAddr) -> ConnectionContext {
        let id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let nic_index = self.next_nic_index.fetch_add(1, Ordering::Relaxed) % 2;
        self.stats.record_inbound(id, peer, nic_index);
        ConnectionContext {
            id,
            nic_index,
            peer,
        }
    }
}

pub async fn run(config: Config, options: RuntimeOptions) -> Result<()> {
    let clients = config
        .nics
        .iter()
        .map(|nic| http::client_for(nic.ip))
        .collect::<Result<Vec<_>>>()?;
    let listener = TcpListener::bind(config.proxy.listen)
        .await
        .with_context(|| format!("failed to bind proxy listener: {}", config.proxy.listen))?;
    let stats = SharedStats::new(&config, options);
    let state = ProxyState {
        config: Arc::new(config),
        clients: Arc::new(clients),
        options,
        stats: stats.clone(),
        next_nic_index: Arc::new(AtomicUsize::new(0)),
        next_connection_id: Arc::new(AtomicU64::new(1)),
    };
    output::spawn_live_renderer(stats);

    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .context("failed to accept connection")?;
        let connection = state.next_connection(peer);
        let state = state.clone();

        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service_state = state.clone();
            let service = service_fn(move |req| {
                let state = service_state.clone();
                let connection = connection;
                async move { Ok::<_, Infallible>(handler::route(req, state, connection).await) }
            });

            if let Err(error) = http1::Builder::new()
                .serve_connection(io, service)
                .with_upgrades()
                .await
            {
                state.stats.record_event(format!(
                    "connection #{} from {} failed: {}",
                    connection.id, connection.peer, error
                ));
            }
            state.stats.finish_pending_tcp(connection.id);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use crate::config::{Config, NicConfig, ProxyConfig};

    use super::*;

    fn test_state() -> ProxyState {
        let config = Config {
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
        };
        let options = RuntimeOptions::default();
        ProxyState {
            config: Arc::new(config.clone()),
            clients: Arc::new(Vec::new()),
            options,
            stats: SharedStats::new(&config, options),
            next_nic_index: Arc::new(AtomicUsize::new(0)),
            next_connection_id: Arc::new(AtomicU64::new(1)),
        }
    }

    #[test]
    fn assigns_nics_round_robin() {
        let state = test_state();
        let peer = SocketAddr::from(([127, 0, 0, 1], 5000));

        assert_eq!(state.next_connection(peer).nic_index, 0);
        assert_eq!(state.next_connection(peer).nic_index, 1);
        assert_eq!(state.next_connection(peer).nic_index, 0);
        assert_eq!(state.next_connection(peer).nic_index, 1);
    }
}
