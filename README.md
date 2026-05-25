# mNIC-CLI(仮称)

## 概要
本プロジェクトは、「ノートPCに2つのUSB外付けNICを搭載し、2つのNICへトラフィックを分散することで、PCの接続帯域を拡張し、冗長性を高めるツール」の開発を行う。
本ツールは、クライアントPC上で動作するプロキシサーバーとして実装する。このサーバーはTCP接続要求を受けるたびに、速度に余裕の有りそうなNICを経由してコネクションを張り、通信を複数経路に分散させる。
しかし、これだけでは、動画サイトのような通信量が多いホストへの通信が割り当てられた1つのNICに通信が集中してしまう。
その対策として、接続要求の接続先(ホスト名)が動画サイトのような通信量が多いサイトの場合は、TLS終端によって通信内容を復号化する。
その後、主要なCDNはAccept-Rangesヘッダに対応するため、Rangeリクエストを用いて両方のNICに通信を割り当てる。その後合体させる。
これにより、通信量が多いホスト名のときのみトラフィックの分散を行うことで、TLS終端処理による遅延を最小化しつつ、通信の高速化と冗長化を実現できると考えている。

## 環境
現時点ではLinuxのみ想定。

## 今後実装予定

- TLS終端
- ホスト名ごとにコネクション数と通信量のログを取る。
- Range-Acceptヘッダを使う
- statsとstateが似ているから改名する。

## セットアップ

1. プロジェクトルートに`config.toml`を作成する。
2. NICのIPアドレスを指定
3. cargo runする
4. ブラウザのHTTPプロキシを`127.0.0.1:8080`に設定



### config.tomlの設定例
```toml
[[nics]]
ip = "192.168.1.10"

[[nics]]
ip = "192.168.1.11"

[proxy]
listen = "127.0.0.1:8080"
```


## 起動

```bash
cargo run
```

現在、Range分割GETはデフォルトで無効。Range分割GETを有効にする場合は`--range-split`を指定する。現在はTLS終端ができないため、http通信のときのみ分割が可能。

```bash
cargo run -- --range-split
```


## ターミナル上の表示

```text
mNIC-CLI live status
Listen: 127.0.0.1:8080    Range split: OFF   Elapsed: 00:03:12

NIC throughput, last 10s average
NIC  IP              TX/s        RX/s        Active  Opened  Failed
[0]  192.168.1.10    12.40 KB/s  4.82 MB/s   2       18      0
[1]  192.168.1.11    10.12 KB/s  4.77 MB/s   1       17      1

NIC totals since start
NIC  IP              TX total    RX total
[0]  192.168.1.10    532.10 KB   245.80 MB
[1]  192.168.1.11    498.20 KB   238.44 MB

Active connections
ID    NIC  Kind      Target                              Age TX        RX State
(none)
Recent events
(none)
```



## ルーティングテーブルの設定

送信元IPを固定しても、実際にどの物理NICから出るかはLinuxのルーティング設定に依存する。ルーティングテーブルを確認する必要がある。

### 使用NICの確認
```bash
ip route get 192.168.24.1 from 192.168.24.10
ip route get 192.168.24.1 from 192.168.24.11
```

### ルーティングの変更
```bash
  sudo ip route add 192.168.10.0/24 dev enxe04f43987bfc src 192.168.24.10 table 121
  sudo ip route add default via 192.168.24.1 dev enxe04f43987bfc src 192.168.24.10 table 121
  sudo ip rule add from 192.168.24.10/32 table 121 priority 121
```