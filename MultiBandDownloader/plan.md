# TCP connection round-robin NIC assignment and live statistics plan

## Goal

このプロキシに以下を追加する。

- HTTPS CONNECTを含むTCPコネクションを、2つのNICへラウンドロビンで割り当てる。
- HTTP Range分割GETは、ラウンドロビンとは独立した機能として扱い、コマンドライン引数で有効/無効を選べるようにする。
- 各NICが送受信したデータ量を計測し、1秒ごとにターミナル上の同じ画面を更新して表示する。
- CSV出力と終了時サマリは作らない。

## Confirmed Decisions

- NIC数はプロトタイプとして必ず2個固定にする。
- CONNECT、通常GET、GET以外のHTTPメソッドは、TCPコネクションに割り当てられたNICを使う。
- Range分割GETは別機能として扱う。
- Range分割GETを使うかどうかはコマンドライン引数で選択する。
- 失敗した接続や途中で切れたCONNECTでも、実際に流れたデータ量は計測に含める。
- ログはスクロールで流し続けない。1秒ごとに既存表示を消して、同じ画面を更新する。
- 直近10秒平均の通信状態と、起動してからの累計を両方表示する。
- リクエストが届いた時、新しい上流コネクションができた時、どちらのNICが使われたか分かる表示にする。
- CSV出力は不要。作らない。
- 終了時サマリも不要。作らない。

## Explicit Proxy Clarification

このプロキシは、ブラウザにHTTPプロキシとして設定して使う明示的プロキシとして実装する。

HTTPSの場合、ブラウザは通常次のようなCONNECTリクエストをプロキシへ送る。

```text
CONNECT example.com:443 HTTP/1.1
```

この方式では、TLS ClientHelloのSNIを読まなくても、CONNECTのauthorityから接続先ホスト名とポートが分かる。
そのため、この計画ではHTTPSの接続先判定にSNIパースは使わない。

## Current State

- `config.toml` の `[[nics]]` は2個固定で検証されている。
- `proxy::run` は `TcpListener::accept()` したブラウザ側TCP接続ごとに `hyper` のHTTP/1 serviceを起動している。
- `ProxyState` はNICごとの `reqwest::Client` を持っている。
- 各 `reqwest::Client` は `local_address()` でNICのIPv4アドレスにバインドされている。
- CONNECTは `TcpSocket::bind()` でNIC0のIPにバインドしている。
- 現在、CONNECT、Range非対応GET、HEAD、GET以外のHTTPメソッドはすべてNIC0固定。
- Range分割GETは、NIC0とNIC1を使って2つのRangeを並列取得するところまで実装済み。
- 現在の出力はリクエストごとの `println!` で、ライブ更新画面ではない。
- NIC別の累積送受信カウンタはまだない。

## Interpretation of TCP Connection Assignment

この計画では「TCPコネクションごとのラウンドロビン」を、プロキシが上流サーバへ接続するときに使うNICを選ぶこととして扱う。

実装上は、ブラウザからプロキシへ来た受信TCP接続を `accept()` した時点でNICを1つ割り当てる。
その接続上で処理されるCONNECTや通常HTTPリクエストは、原則として同じ割り当てNICを使う。

理由:

- `accept()` が現在のコードで一番明確なTCPコネクション境界になっている。
- CONNECTでは、1つのブラウザ側TCP接続が1つの上流TCP接続に対応する。
- HTTP/1.1 keep-aliveで同じブラウザ側TCP接続に複数リクエストが流れても、「そのTCPコネクションは同じNICを使う」と説明できる。

Range分割GETだけは例外にする。
Range分割GETが有効な場合、1つのGETを2本の上流Range GETに分けるため、既存機能どおりNIC0とNIC1を同時に使う。

## Command Line Flags

新しい依存関係を増やさず、まずは `std::env::args()` で最小限に実装する。

想定する引数:

```bash
cargo run
cargo run -- --range-split
cargo run -- --no-range-split
```

デフォルトは分かりやすさを優先し、Range分割GETを無効にする。

- `--range-split`: Range対応GETを2NIC分割する。
- `--no-range-split`: Range対応GETでも分割せず、割り当てNICで通常GETする。

不明な引数が渡された場合は、起動時にエラーを出して終了する。

## NIC Round-Robin Design

`ProxyState` にラウンドロビン用のセレクタを追加する。

```rust
struct NicSelector {
    next: AtomicUsize,
}
```

NIC選択:

```rust
let index = next.fetch_add(1, Ordering::Relaxed) % 2;
```

2NIC固定なので、返る値は `0, 1, 0, 1...` になる。

`proxy::run` の `accept()` 直後にNIC indexを決める。
そのindexを `handler::route(req, state, connection_id, assigned_nic_index)` のように渡す。

使用方針:

- CONNECT: 割り当てNICのIPで `TcpSocket::bind()` して上流へ接続する。
- `--no-range-split` のGET: 割り当てNICの `reqwest::Client` を使う。
- Range非対応GET: 割り当てNICの `reqwest::Client` を使う。
- HEAD: 割り当てNICの `reqwest::Client` を使う。
- GET以外: 割り当てNICの `reqwest::Client` を使う。
- `--range-split` でRange可能なGET: 既存どおりNIC0/NIC1で並列Range GETする。

## Reqwest Connection Reuse

回答が「実装が簡単な方」なので、まずは既存のNIC別 `reqwest::Client` を維持する。

ただし、`reqwest::Client` は内部で接続プールを持つ。
厳密に「上流TCP接続1本ごとに必ず新しくラウンドロビンしたい」という実験になった場合は、後で以下を検討する。

- `pool_max_idle_per_host(0)` でアイドル接続再利用を抑える。
- リクエストごとにClientを作る。

今回は実装容易性を優先し、既存Clientを使い回す。

## Data Accounting Policy

「実装が容易な方法」という回答を踏まえ、まずはアプリケーション層でプロキシが実際に読み書きしたpayload量を数える。

含める:

- CONNECTトンネルでブラウザから上流へ流れたバイト数
- CONNECTトンネルで上流からブラウザへ流れたバイト数
- HTTPリクエストbodyのバイト数
- HTTPレスポンスbodyのバイト数
- Range GETで各NICが受信したchunk bodyのバイト数
- 失敗または途中切断までに流れたバイト数

含めない:

- TCP/IPヘッダ
- Ethernetヘッダ
- TCP再送
- ARPなどHTTP/CONNECT以外の通信
- HTTPヘッダ
- TLSレコードヘッダ

表示上の意味:

- `TX`: プロキシから上流サーバ方向へ送ったpayload量
- `RX`: 上流サーバからプロキシ方向へ受け取ったpayload量

この値はOSやNICドライバが持つ実インターフェースの `tx_bytes` / `rx_bytes` とは完全には一致しない。
主な差分はTCP/IPヘッダ、Ethernetヘッダ、TLSレコードヘッダ、再送分などを含めないことによる。
全く別物を数えるわけではなく、このプロキシが扱った通信の本体データ量として解釈する。

## Live Statistics Design

`src/stats.rs` を追加し、NIC別・接続別の状態を集約する。

主な構造:

```rust
struct Stats {
    started_at: Instant,
    nics: [NicStats; 2],
    active_connections: HashMap<u64, ConnectionStats>,
    recent_events: VecDeque<Event>,
}

struct NicStats {
    ip: Ipv4Addr,
    total_tx: u64,
    total_rx: u64,
    last_second_tx: u64,
    last_second_rx: u64,
    active_connections: usize,
    opened_connections: u64,
    failed_connections: u64,
}
```

並行処理から更新するため、まずは `Arc<Mutex<Stats>>` で実装する。
1秒ごとの画面更新と、通信処理側のカウンタ更新が同時に起きても破綻しないようにする。

高頻度更新が重くなった場合だけ、後で `AtomicU64` とevent channelに分ける。
プロトタイプでは `Mutex` の方が読みやすく安全。

## Real-Time Counting

1秒ごとの「現在の通信状態」を出すため、完了時にまとめて加算するだけでは不足する。
そのため、以下の箇所は読み書きのchunkごとにカウンタを更新する。

### CONNECT

現在の `tokio::io::copy_bidirectional` は、終了時までバイト数が返らない。
ライブ表示には向かないため、手動の双方向コピーに置き換える。

- browser -> server を読むたびに、割り当てNICの `TX` を加算する。
- server -> browser を読むたびに、割り当てNICの `RX` を加算する。
- どちらかが終了またはエラーになったら接続を閉じる。
- エラー終了でも、それまでに加算済みのバイト数は残す。

### HTTP GET / Other Methods

`reqwest::Response::bytes_stream()` を使い、受信chunkごとに `RX` を加算する。

ブラウザへ返す処理は現状と同じく一度メモリに集めてから返す。
ただし、上流から読み込む途中でカウンタは更新する。

リクエストbodyがあるメソッドでは、ブラウザから読み取ったbody長を、上流送信時に `TX` として加算する。

### Range GET

各Range GETの受信chunkごとに、担当NICの `RX` を加算する。

- 前半chunk: NIC0の `RX`
- 後半chunk: NIC1の `RX`

Range GETのリクエストbodyはないので、通常は `TX` は増えない。

## Terminal Screen Design

画面は1秒ごとに再描画する。
通常の `println!` でログを流すのではなく、ANSI escape sequenceでカーソルを先頭へ戻し、画面を消してから同じレイアウトを描く。

実装方針:

- 起動時に描画タスクを `tokio::spawn` する。
- 1秒ごとに `Stats` のsnapshotを取り、stdoutへ描画する。
- 描画には `print!("\x1b[2J\x1b[H")` を使う。
- 通常のリクエストログ `println!` はライブ画面と競合するため、イベント記録に置き換える。
- 致命的な起動エラーだけは従来通り標準エラーへ出す。

表示案:

```text
mNIC-CLI live status
Listen: 127.0.0.1:8080    Range split: OFF   Elapsed: 00:03:12
Proxy mode: explicit HTTP proxy. HTTPS target is taken from CONNECT authority.

NIC throughput, last 10s average
NIC  IP              TX/s        RX/s        Active  Opened  Failed
[0]  192.168.1.10    12.40 KB/s  4.82 MB/s   2       18      0
[1]  192.168.1.11    10.12 KB/s  4.77 MB/s   1       17      1

NIC totals since start
NIC  IP              TX total    RX total
[0]  192.168.1.10    532.10 KB   245.80 MB
[1]  192.168.1.11    498.20 KB   238.44 MB

Active connections
ID    NIC  Kind      Target                              Age    TX        RX        State
42    [1]  CONNECT   www.example.com:443                 00:14  8.20 KB   1.24 MB   open
43    [0]  HTTP GET  http://raspberrypi.local/10mb.bin    00:02  0 B       3.11 MB   full

Recent events
20:15:01  inbound #42 from 127.0.0.1:51902 assigned NIC[1] 192.168.1.11
20:15:01  CONNECT #42 www.example.com:443 opened via NIC[1]
20:15:03  inbound #43 from 127.0.0.1:51906 assigned NIC[0] 192.168.1.10
20:15:03  GET #43 http://raspberrypi.local/10mb.bin range split started
20:15:04  CONNECT #41 closed after 23.40s tx 12.10 KB rx 8.30 MB
```

表示ルール:

- `NIC throughput, last 10s average` は直近10秒間に増えたTX/RXを平均速度として表示する。
- `NIC totals since start` は起動後の累計を表示する。
- `Active connections` は現在開いている接続だけを表示する。
- 行数が増えすぎる場合、Active connectionsは先頭10件程度に制限する。
- `Recent events` は直近5から10件だけ表示する。
- 過去の表示は毎秒消えるので、ログがスクロールし続けない。

## Output Event Timing

以下のタイミングでeventを追加する。

- ブラウザからプロキシへTCP接続が届いた時
- その接続へNICが割り当てられた時
- CONNECTの上流TCP接続を作る時
- CONNECTが閉じた時
- HTTP GETを受けた時
- Range splitが開始された時
- Range splitが成功または失敗した時
- Range非対応などで通常GETへフォールバックした時
- GET以外のHTTPメソッドを転送した時
- 上流接続やHTTP処理が失敗した時

## Implementation Steps

1. CLI引数パーサを追加し、`range_split_enabled` を起動設定として持つ。
2. `ProxyState` に以下を追加する。
   - CLI起動設定
   - NICラウンドロビンセレクタ
   - connection id発行用カウンタ
   - `Arc<Mutex<Stats>>`
3. `proxy::run` の `accept()` 時にconnection idとNIC indexを決め、eventへ記録する。
4. `handler::route` にconnection idとNIC indexを渡す。
5. CONNECTで割り当てNICを使うように変更する。
6. CONNECTの `copy_bidirectional` を、chunkごとにStatsへ加算する手動コピーへ置き換える。
7. `--no-range-split` の場合は、Range対応GETでも割り当てNICで通常GETする。
8. `--range-split` の場合は、既存どおりHEAD判定後にNIC0/NIC1でRange分割GETする。
9. `http::get_range` / `http::get_full` / `http::request_with_body` を、受信chunkごとにカウンタ更新できる形に変更する。
10. 既存の `output::print_*` はライブ画面と競合するため、event追加またはStats更新へ置き換える。
11. 1秒ごとにライブ画面を再描画するタスクを追加する。
12. 単体テストを追加する。
13. `cargo fmt --check` と `cargo test` を実行する。
14. 必要なら `README.md` と `specification.md` にCLI引数とライブ表示の説明を追記する。

## Tests

最低限追加するテスト:

- 2NICラウンドロビンが `0, 1, 0, 1...` の順で返ること。
- CLI引数なしならRange分割OFF、`--range-split` ならON、`--no-range-split` ならOFFになること。
- 不明なCLI引数で起動設定エラーになること。
- StatsにTX/RXが加算されること。
- 直近10秒平均の差分計算が累計と独立していること。
- 失敗イベントでも、加算済みデータ量が消えないこと。

手動検証:

- `--range-split` でRange対応HTTPファイルを取得し、NIC0/NIC1両方のRXが増えること。
- `--no-range-split` で同じファイルを取得し、割り当てNICだけのRXが増えること。
- HTTPSサイトへアクセスし、CONNECTがNIC0/NIC1へ交互に割り当てられること。
- 通信中にターミナル表示がスクロールせず、同じ画面内で更新されること。

実行コマンド:

```bash
cargo fmt --check
cargo test
```

## Routing Risk and Mitigation

`local_address()` や `TcpSocket::bind()` は、アプリケーションが使う送信元IPアドレスを固定する。
ただしLinuxでは、最終的にどの物理NICからパケットが出るかはルーティングテーブルで決まる。
つまり、送信元IPを `192.168.1.10` にしても、OSの経路設定が不正なら期待したNICから出ない可能性がある。

これは実験結果を壊す大きなリスクなので、以下を実施する。

- 起動時に、2つのNIC IPとlistenアドレスをライブ画面に表示する。
- 接続失敗時は、bindに失敗したのか、DNSに失敗したのか、connectに失敗したのかをeventへ明確に出す。
- READMEまたは仕様に、実験前に以下のコマンドで経路確認する手順を追加する。

```bash
ip route get <target-ip> from <nic0-ip>
ip route get <target-ip> from <nic1-ip>
```

期待する状態は、それぞれ別の `dev <interface>` が表示されること。
同じ `dev` が表示される場合、アプリケーション側ではなくOSのルーティング設定を直す必要がある。

必要になった場合の追加策として、Linux限定の `SO_BINDTODEVICE` を検討する。
ただしこれは権限が必要になり実装も環境依存になるため、今回の標準実装には含めない。

リスクを理解しました。とりあえずルーティング設定を変更することで解決できる問題なのでしたらとりあえずは今はリスクを許容します。ただし、実験の正確性の観点からも正しく指定したNICで通信できているかを後で確認したいので、確認することを忘れないように、READMEの先頭にリマインドしておいてください。

## Accepted Risks

- `reqwest::Client` の接続プールによる上流TCP接続再利用は、実装容易性を優先して許容する。
- Range GETは既存どおりレスポンス全体をメモリに載せてから返す。メモリは潤沢にある前提なので許容する。
- 1秒未満で完了する短い接続がActive connectionsに表示されないことは許容する。Recent eventsと累計には反映する。
- payload量と実NICパケット量は完全一致しないが、差分は主にヘッダ・再送・プロキシ外通信分であり、このプロキシが扱った通信量の観察値として使う。

## Output Integrity

ライブ画面の表示崩れを避けるため、stdoutへ通常ログを直接出さない。

- ライブ描画タスクだけがstdoutへ書き込む。
- リクエスト処理・CONNECT処理・エラー処理は `println!` しない。
- 通常ログ相当の情報は `Stats` の `recent_events` に追加し、次回描画で表示する。
- 起動失敗などライブ画面開始前の致命的エラーだけstderrへ出す。

## Final Decisions

- 明示的HTTPプロキシとして実装する。
- HTTPS接続先はCONNECT authorityから取得する。
- SNIパースは実装しない。
- Range分割GETはデフォルトOFFにする。
- `--range-split` を指定した時だけRange分割GETを有効にする。
