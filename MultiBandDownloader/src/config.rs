use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub nics: Vec<NicConfig>,
    pub proxy: ProxyConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NicConfig {
    pub ip: Ipv4Addr,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    pub listen: SocketAddr,
}

pub fn load_config(path: &Path) -> Result<Config> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    let config: Config = toml::from_str(&raw)
        .with_context(|| format!("failed to parse config file: {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<()> {
    if config.nics.len() != 2 {
        bail!(
            "config must contain exactly 2 [[nics]] entries, got {}",
            config.nics.len()
        );
    }

    if !config.proxy.listen.ip().is_loopback() {
        eprintln!(
            "[WARN] proxy.listen is not loopback: {}",
            config.proxy.listen
        );
    }

    Ok(())
}
