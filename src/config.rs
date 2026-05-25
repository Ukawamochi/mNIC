use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};
use anyhow::{Context, Result, bail};//bali:エラーを即座に返して関数を抜ける
use serde::Deserialize;//ファイル上のテキストデータを、メモリ上に文字列として読み込む


#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub nics: Vec<NicConfig>,//xxx.xxx.xxx.xxx
    pub proxy: ProxyConfig,//xxx.xxx.xxx.xxx:xxxx
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
    //ファイルから読み込んだテキストをメモリ上に文字列として保存する。"[[nics]]\nip = \"192.168.1.10\"\n[[nics]]\n..."みたいになっている。
    let raw :String = fs::read_to_string(path).with_context(|| format!("failed to read config file: {}", path.display()))?;
    //Config型にパースする。
    let config: Config = toml::from_str(&raw).with_context(|| format!("failed to parse config file: {}", path.display()))?;
    validate_config(&config)?;//nicが2つあるか、ループバックされているか検証する
    Ok(config)
}

fn validate_config(config: &Config) -> Result<()> {
    //NICを2つ指定していることを検証
    if config.nics.len() != 2 {
        bail!(
            "config must contain exactly 2 [[nics]] entries, got {}",
            config.nics.len()
        );
    }
    //外部から接続できないことを確認する。同じLANからもアクセスはできない仕様。
    if !config.proxy.listen.ip().is_loopback() {
        eprintln!(//stderrに出力
            "[WARN] proxy.listen is not loopback: {}",
            config.proxy.listen
        );
    }
    //():ユニット型。
    Ok(())
}
