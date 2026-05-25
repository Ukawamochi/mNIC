mod chunk;
mod cli;
mod config;
mod handler;
mod http;
mod output;
mod proxy;
mod stats;

use std::path::Path;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let options = cli::parse_args(std::env::args().skip(1))?;
    let config = config::load_config(Path::new("config.toml"))?;//Pathを引数に渡す
    proxy::run(config, options).await
}
