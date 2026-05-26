use std::{
    convert::Infallible,//絶対エラーが起こらないことを示す型
    net::SocketAddr,//x.x.x.x:xxxxみたいなIPアドレスの型
    sync::{
        Arc,//一つの変数に複数の所有者で共有するためのポインタ
        //同時に同じ所に書き込まないように、読む→上書きを分割できないセット操作にする
        atomic::{
            AtomicU64,//AtomicU64型を追加
            AtomicUsize,//AtomicUsize型を追加
            Ordering,//実行の順序をどれくらい固定するか
        },
    },
};
//エラー型をanyhow::Errorに統一し、Contextで発生箇所などの説明を付け足す。
use anyhow::{Context, Result};

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;//tokioのTCPStreamをHyperが扱えるように変換する
use tokio::net::TcpListener;

//crate内のモジュール
use crate::{cli::RuntimeOptions, config::Config, handler, http, output, stats::SharedStats};

#[derive(Clone)]//自分で作ったStructの型にCloneを実装する
pub struct ProxyState {
    pub config: Arc<Config>,
    pub clients: Arc<Vec<reqwest::Client>>,
    pub options: RuntimeOptions,
    pub stats: SharedStats,
    next_nic_index: Arc<AtomicUsize>,
    next_connection_id: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectionContext {//値が全てスタックに収まる型なので、Copyを実装する。
    pub id: u64,
    pub nic_index: usize,
    pub peer: SocketAddr,
}

impl ProxyState {
    fn next_connection(&self, peer: SocketAddr) -> ConnectionContext {
        let id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);//connection_idを1増やす
        let nic_index = self.next_nic_index.fetch_add(1, Ordering::Relaxed) % 2;//NICを切り替える
        self.stats.record_inbound(id, peer, nic_index);
        ConnectionContext {
            id,
            nic_index,
            peer,
        }
    }
}
//config: コマンドライン引数から作ったoptions構造体を渡す(cli.rs)
// options: config.tomlの内容をtomlとしてパースしたもの(config.rs)
pub async fn run(config: Config, options: RuntimeOptions) -> Result<()> {
    //NICごとにHttpClientsを作成
    let clients = config
        .nics
        .iter()
        .map(|nic| http::client_for(nic.ip))
        .collect::<Result<Vec<_>>>()?;
    //プロキシのListenするソケットを作成。
    let listener = TcpListener::bind(config.proxy.listen)
        .await
        .with_context(|| format!("failed to bind proxy listener: {}", config.proxy.listen))?;
    let stats = SharedStats::new(&config, options);//統計情報//STATS.rsを参照
    let state = ProxyState {
        config: Arc::new(config),//NICのIP、Listen中のアドレスとソケット
        clients: Arc::new(clients),//上流
        options,
        stats: stats.clone(),//統計情報を共有
        next_nic_index: Arc::new(AtomicUsize::new(0)),//使用NICを決定
        next_connection_id: Arc::new(AtomicU64::new(1)),//TCP接続一本ごとにconnection_idを割り当て
    };
    //統計情報をターミナルに表示する処理を別スレッドで実行する
    output::spawn_live_renderer(stats);//OUTPUT.rsを参照

    loop {
        //Listenerにブラウザからの接続が来るのを待つ。
        // stream:ブラウザとプロキシ間のTCP接続,peer:ブラウザのIPとポート番号
        let (stream, peer) = listener
            .accept()
            .await
            .context("failed to accept connection")?;
        let connection = state.next_connection(peer);//NICを切り替え、connection_idを1増やしたものを割り当てる。
        let state = state.clone();//ArcでCloneするのでstateを所有権ごと渡せる

        //非同期でそのTCPコネクションの通信を処理する。
        // async moveはstream,connection,stateをmoveしている。(以下のループは寿命が不明だから所有権ごと渡す)
        tokio::spawn(async move {
            let io = TokioIo::new(stream);//streamはtokioの機能、hyperに対応させるためにTokioIOを使う。
            let service_state = state.clone();//stateをCloneする
            
            let service = service_fn(move |req| {
                let state = service_state.clone();
                let connection = connection;
                //handler::routeはエラーもレスポンスに含めるのでエラーは発生しない。
                //ターボフィッシュの構文:Ok::<成功側の型(_なので推論), エラー側の型(今回はエラーが起こらない)>(値)
                // 通信に失敗してもStatusコードをhttpでブラウザに送る。Rustではエラーを処理しない
                async move { Ok::<_, Infallible>(handler::route(req, state, connection).await) }
            });
            //エラー発生時にはエラーを統計情報に記録。
            if let Err(error) = http1::Builder::new()
                .serve_connection(io, service)
                .with_upgrades()
                .await
            {
                state.stats.record_event(format!(//format!は値を埋め込んだStringに加工するマクロ
                    "connection #{} from {} failed: {}",
                    connection.id, connection.peer, error
                ));
            }
            //統計情報を更新する
            state.stats.finish_pending_tcp(connection.id);
        });
    }
}
//testのときだけコンパイルをする。runのときはバイナリに含まない。
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
