すまない。スコープは絞りつつ、実装に必要な情報は厚く書く。作り直す。

---

# mNIC-CLI: 実験用プロキシ仕様書

## 0. 現行実装メモ

この文書の詳細仕様には、初期案として「Range対応GETは常に2NIC分割する」「CONNECTや通常転送はNIC1固定」「リクエストごとにログを流す」という古い記述が残っている。
現行実装では、以下を優先する。

- 明示的HTTPプロキシとして動作する。HTTPS接続先はSNIではなくCONNECT authorityから取得する。
- TCP接続ごとにNIC 0、NIC 1、NIC 0...の順でラウンドロビン割り当てする。
- Range分割GETはデフォルトOFF。`--range-split`指定時のみ有効にする。
- Range分割OFF時は、HTTP GETでもHEAD判定をせず、割り当てNICで通常GETする。
- ターミナル出力はリクエストごとのスクロールログではなく、1秒ごとに同じ画面を更新するライブ表示にする。
- 表示するTX/RXはプロキシが扱ったpayload量であり、TCP/IPヘッダ、Ethernetヘッダ、TLSレコードヘッダ、再送分などは含まない。
- CSV出力と終了時サマリは実装しない。

## 1. 目的

HTTP Range Requestによる2NIC並列ダウンロードが、Webブラウジングを実際に高速化するかを検証する実験用プロキシツール。1MB〜100MB級のリソースに対して常に2等分割を適用し、どのような条件で高速化が成立するかを観察するためのデータを取得する。

本ソフトウェア自体は最終プロダクトではなく、実証実験のためのプロトタイプである。よって機能は最小限にとどめ、計測のための観察可能性を優先する。

## 2. 全体動作

ユーザーはブラウザのHTTPプロキシ設定を`localhost:8080`（設定可能）に向ける。以降、ブラウザのHTTP/HTTPSリクエストがこのプロキシを経由する。本MVPはブラウザ用HTTPプロキシに限定し、URLを指定して直接ダウンロードするCLI機能は持たない。

プロキシは以下のように振る舞う：

- **HTTP GET**：HEADリクエストでサイズと`Accept-Ranges`を確認し、Range対応サーバには2NIC並列で2等分割取得、非対応なら単一NICで取得
- **HTTPS（CONNECTメソッド）**：素通しトンネル（TLS終端はせず、実験対象外。ブラウジングを壊さないため）
- **GET・CONNECT以外のHTTPメソッド**：単一NICで通常転送（POST等、ブラウジング全般を動作させるため）

各リクエストの結果はターミナルに人間可読形式で出力する。ユーザーはこれをコピーして応募書類等に使う。

## 3. 設定ファイル

プロジェクトルートに`config.toml`を配置する。プログラム起動時にカレントディレクトリから読み込む。

### 3.1 形式

```toml
[[nics]]
ip = "192.168.1.10"

[[nics]]
ip = "192.168.1.11"

[proxy]
listen = "127.0.0.1:8080"
```

### 3.2 検証

- `nics`配列は要素数2を前提とする。0個または3個以上ならエラーで起動失敗
- 各IPアドレスは有効なIPv4文字列であること
- `listen`はSocketAddrとしてパース可能であること

設定ファイルが存在しない、または不正な場合はエラーメッセージを出して起動失敗。

## 4. プロキシ動作詳細

### 4.1 サーバ起動

`hyper`を用いて`config.proxy.listen`でHTTPサーバを起動する。各受信接続は`tokio::spawn`で並列処理する。`hyper::server::conn::http1::Builder::new().serve_connection()`を使用する。

### 4.2 リクエスト振り分け

リクエストの`Method`によって処理を分岐：

```rust
match req.method() {
    &Method::CONNECT => handle_connect(req, &nics[0]).await,
    &Method::GET     => handle_get(req, &nics).await,
    _                => handle_passthrough(req, &nics[0]).await,
}
```

`nics[0]`を「デフォルトNIC」として使う。CONNECTやその他のメソッドではNIC選択の最適化はしない。

ブラウザから受け取った通常ヘッダは可能な範囲で上流リクエストへ転送する。ただし、`Connection`、`Proxy-Connection`、`Transfer-Encoding`、`Keep-Alive`、`Upgrade`、`TE`、`Trailer`などのhop-by-hopヘッダは転送しない。

### 4.3 HTTP GET処理（本実験の主対象）

#### 4.3.1 URL抽出

明示的プロキシでは、リクエスト行に絶対URLが含まれる：

```
GET http://example.com/path/to/file HTTP/1.1
```

`req.uri()`から完全なURLを取得する。HTTPSの場合はCONNECTで処理されるためここには来ない。

#### 4.3.2 HEADリクエスト

NIC1（`nics[0]`）から対象URLにHEADリクエストを送信する。`reqwest::Client`を`local_address(IpAddr::V4(nics[0].ip))`でバインドして使用。

Range分割対象のHEAD/GETでは`Accept-Encoding: identity`を送信し、非圧縮レスポンスを要求する。圧縮転送への対応は本MVPでは扱わない。

取得すべきヘッダ：

- `Content-Length`：Range分割には必須。なければ単一NICフォールバック
- `Accept-Ranges`：`bytes`なら対応、`none`または欠落なら非対応扱い

リダイレクト（301/302/307/308）はHTTP URLの範囲で自動追従する（最大10回まで）。最終URLを以降のGETに使用する。HTTPS URLへリダイレクトされる場合は追従せず、HTTPSはブラウザ側のCONNECTに任せる。

HEADが失敗した場合は、その理由をターミナルに記録し、Range判定不能としてNIC1の通常GETにフォールバックする。

#### 4.3.3 取得方式の決定

```
if HEAD成功 && Content-Lengthあり && size >= 2 && Accept-Ranges == "bytes":
    → Range分割2NIC並列取得
else:
    → NIC1で通常GET（単一NICフォールバック）
```

サイズによる性能上の閾値判定はしない。**Range対応なら常に2分割**。ただし`size < 2`の場合は分割不能なので単一NICフォールバックとする。これは実験条件として小サイズも含めて挙動を観察するため。

#### 4.3.4 Range分割2NIC並列取得

ファイルサイズを`size`バイトとし、`size >= 2`の場合に以下のように分割：

- **NIC1の範囲**：`0` から `(size / 2) - 1`
- **NIC2の範囲**：`size / 2` から `size - 1`

`size`が奇数の場合、後半が1バイト多くなる。問題ない。

各NICで対応するIPアドレスを`local_address`としてバインドした`reqwest::Client`を作る。それぞれ`Range: bytes={start}-{end}`ヘッダを付けてGETリクエストを送信する。

両方のリクエストを`tokio::join!`で並列実行し、両方の完了を待つ。

期待されるレスポンス：

- ステータス：206 Partial Content
- `Content-Range: bytes {start}-{end}/{total}`ヘッダ

これらを検証し、不一致ならエラー。

#### 4.3.5 ボディの結合とブラウザへの返送

両方のレスポンスボディを完全に受信した後、バイト列として連結する。連結結果はブラウザへ`200 OK`で返す（206ではない、ブラウザはRangeを要求していないため）。

レスポンスヘッダはNIC1の応答を元にする。ただし安全のため、ブラウザへ返すヘッダはホワイトリスト方式で選別する。`Content-Type`、`Content-Encoding`、`ETag`、`Last-Modified`、`Cache-Control`等の必要なヘッダだけを転送し、以下は調整または除外する：

- `Content-Length`：連結後の合計（=`size`）
- `Content-Range`：含めない（200応答のため）
- `Transfer-Encoding`、`Connection`、`Proxy-Connection`、`Keep-Alive`、`Upgrade`、`TE`、`Trailer`：含めない

ストリーミング転送は実装しない。両方の取得完了後にまとめて返す。

#### 4.3.6 単一NICフォールバック

Range非対応、HEAD失敗、`Content-Length`欠落、`size < 2`の場合、NIC1で通常GETを実行し、レスポンスをブラウザへ転送する。ボディは全受信してから返す（ストリーミング転送なし）。レスポンスヘッダはRange分割時と同様にhop-by-hopヘッダを除外し、必要に応じて`Content-Length`を再計算する。

### 4.4 CONNECT処理（HTTPSトンネル）

ブラウザが`CONNECT www.example.com:443 HTTP/1.1`を送ってきたとき：

1. `req.uri().authority()`から宛先ホスト・ポートを取得
2. `tokio::net::lookup_host()`でDNS解決し、IPv4アドレスを選ぶ
3. NIC1のIPをローカルアドレスにバインドした`TcpSocket`で宛先に接続
4. ブラウザに`HTTP/1.1 200 Connection Established\r\n\r\n`を返す
5. `hyper::upgrade::on(req).await`でブラウザ側のTCPストリームを取得
6. ブラウザ側ストリームと宛先サーバ側ストリームの間で`tokio::io::copy_bidirectional`を実行
7. どちらかがクローズされるまで継続

DNS解決結果にIPv4アドレスがない場合はCONNECTを失敗させる。IPv6対応は本MVPでは扱わない。

NICへのバインドは`TcpSocket::bind`を使用する：

```rust
let socket = TcpSocket::new_v4()?;
socket.bind(SocketAddr::new(nic1_ip, 0))?;
let stream = tokio::time::timeout(Duration::from_secs(10), socket.connect(target_addr)).await??;
```

CONNECT処理はターミナルに最小限の情報のみ出力する（後述）。

### 4.5 その他メソッド処理

POST、PUT、DELETE等のGETでもCONNECTでもないリクエストは、NIC1で通常転送する。ボディがあればそのまま転送する。レスポンスもそのまま返す。

実装は`reqwest`のメソッドAPIを使う。

## 5. ターミナル出力仕様


### 5.1 GETリクエスト（Range分割成功時）

```
[GET]  http://raspberrypi.local/test-10mb.bin
       Size: 10.00 MB, Range: yes
       NIC 192.168.1.10: bytes 0-5242879        in 5.21s (1.01 MB/s)
       NIC 192.168.1.11: bytes 5242880-10485759 in 5.18s (1.01 MB/s)
       Total: 5.21s, aggregate 1.92 MB/s
```

各フィールドの意味：

- 1行目：メソッドとURL
- 2行目：HEADで取得したサイズ（人間可読）と、Range対応有無
- 3〜4行目：各NICの担当範囲、所要時間、その範囲だけの実効スループット
- 5行目：全体所要時間（=遅い方のNICの時間）と合計スループット

### 5.2 GETリクエスト（Range非対応・単一NIC）

```
[GET]  http://raspberrypi.local/dynamic.html
       Size: 12.30 KB, Range: no
       NIC 192.168.1.10: full transfer          in 0.15s (82.0 KB/s)
       Total: 0.15s
```

### 5.3 CONNECT

```
[CONNECT] www.google.com:443  (tunneled, NIC 192.168.1.10)
```

トンネルが閉じた時点で1行追加：

```
          closed after 23.4s
```

### 5.4 その他メソッド

```
[POST] http://api.example.com/endpoint
       NIC 192.168.1.10: 0.34s
```

### 5.5 エラー時

```
[GET]  http://raspberrypi.local/missing.bin
       HEAD failed (404 Not Found), falling back to single NIC
       NIC 192.168.1.10: full transfer          in 0.12s
```

```
[GET]  http://raspberrypi.local/test.bin
       Size: 10.00 MB, Range: yes
       NIC 192.168.1.10: completed in 5.20s
       NIC 192.168.1.11: ERROR: connection refused
       FAILED: returning 502
```

## 6. エラーハンドリング

### 6.1 エラー方針

```rust
type Result<T> = anyhow::Result<T>;
```

専用エラー型は定義せず、`anyhow::Result`と`anyhow::Context`を使って失敗箇所の文脈を付ける。

HEAD失敗、`Content-Length`欠落、Range非対応、`size < 2`はエラーではなく単一NICフォールバックとして扱う。Range分割開始後に片方のchunkが失敗した場合、またはCONNECT接続に失敗した場合は、そのリクエストを失敗としてブラウザに502 Bad Gatewayを返す。プロキシ自体は停止させない。

### 6.2 起動時メッセージ

```
mNIC-CLI starting
  NICs:
    [0] 192.168.1.10
    [1] 192.168.1.11
  Listen: 127.0.0.1:8080
Ready.
```

## 7. アーキテクチャ

### 7.1 ファイル構成

```
src/
├── main.rs       # エントリポイント、起動処理
├── config.rs     # config.toml読み込みとバリデーション
├── proxy.rs      # hyperサーバ、リクエスト振り分け
├── handler.rs    # GET/CONNECT/その他のハンドラ
├── http.rs       # reqwestクライアント生成、HEAD/GET/Rangeリクエスト
├── chunk.rs      # 2等分割の計算
└── output.rs     # ターミナル出力フォーマット
```

### 7.2 主要な型定義

```rust
// config.rs
#[derive(Debug, Deserialize)]
pub struct Config {
    pub nics: Vec<NicConfig>,
    pub proxy: ProxyConfig,
}

#[derive(Debug, Deserialize)]
pub struct NicConfig {
    pub ip: Ipv4Addr,
}

#[derive(Debug, Deserialize)]
pub struct ProxyConfig {
    pub listen: SocketAddr,
}

// chunk.rs
pub struct Chunk {
    pub start: u64,
    pub end: u64,  // inclusive
}

pub fn split_in_half(total_size: u64) -> [Chunk; 2] {
    assert!(total_size >= 2);
    let mid = total_size / 2;
    [
        Chunk { start: 0, end: mid - 1 },
        Chunk { start: mid, end: total_size - 1 },
    ]
}

// http.rs
pub struct ServerInfo {
    pub content_length: Option<u64>,
    pub accepts_ranges: bool,
    pub final_url: String,
}

pub async fn head(client: &reqwest::Client, url: &str) -> anyhow::Result<ServerInfo>;

pub async fn get_range(
    client: &reqwest::Client,
    url: &str,
    chunk: &Chunk,
) -> anyhow::Result<(Vec<u8>, Duration)>;

pub async fn get_full(
    client: &reqwest::Client,
    url: &str,
) -> anyhow::Result<(Vec<u8>, Duration, HeaderMap)>;
```

### 7.3 処理フロー

```
main()
 → load_config()
 → start_proxy_server()
   → for each connection:
     tokio::spawn(handle_connection)
       → for each request:
         match method:
           CONNECT → handle_connect()
           GET     → handle_get()
                       → http::head()
                       → if head ok && content_length exists && size >= 2 && range supported:
                           parallel:
                             - http::get_range(nic1, chunk1)
                             - http::get_range(nic2, chunk2)
                           combine bytes
                         else:
                           http::get_full(nic1)
                       → output::print_get_result()
                       → return response to browser
           other   → handle_passthrough()
```

## 8. 依存クレート

```toml
[package]
name = "mnic-cli"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1", features = ["full"] }
hyper = { version = "1", features = ["full"] }
hyper-util = { version = "0.1", features = ["full"] }
http-body-util = "0.1"
reqwest = { version = "0.12", default-features = false, features = ["stream", "rustls-tls"] }
toml = "0.8"
serde = { version = "1", features = ["derive"] }
anyhow = "1"
futures-util = "0.3"
bytes = "1"
```

HTTPSリソースに対するHEAD/Range分割は本MVPでは実装しない。HTTPSはCONNECTで完全に素通しする。

## 9. 実装順序

CodingAgentが段階的に実装することを想定し、以下の順で進める。各段階で動作確認可能な状態にする。

### Phase 1: 設定読み込み
- `config.toml`を読み込んで内容を表示するだけのプログラム
- バリデーション実装

### Phase 2: プロキシ最小骨格
- hyperでHTTPサーバを起動
- 任意のリクエストに対して`200 OK`を返すだけ
- ブラウザのプロキシ設定をlocalhost:8080にして、何かが返ってくることを確認

### Phase 3: GETの単一NIC転送
- GETリクエストを受けたら、URL抽出してreqwestで取得
- reqwestは`local_address`でNIC1にバインド
- レスポンスをそのままブラウザへ返す
- ブラウザでhttp://raspberrypi.local/にアクセスして表示できることを確認

### Phase 4: CONNECTトンネル
- HTTPSサイトにブラウザがアクセスできるようにする
- `tokio::io::copy_bidirectional`でトンネル
- ブラウザでhttps://www.google.com/にアクセスできることを確認

### Phase 5: HEADとRange判定
- GETリクエスト時、まずHEADを送って情報取得
- ターミナルにサイズとRange対応を出力
- ただしこの段階ではまだ単一NICで取得

### Phase 6: Range分割並列取得
- Range対応の場合、2NICで2等分割並列取得
- ボディを連結してブラウザへ返す
- ターミナル出力を整形

### Phase 7: その他メソッドとエラー処理
- POST等の単純転送
- 各種エラーをターミナル出力＋ブラウザに502返送

### Phase 8: 出力整形と仕上げ
- ターミナル出力を仕様5節通りに整える
- 起動メッセージ
- README.md作成

## 10. 動作確認手順（実装完了後）

プロキシ起動：

```bash
cargo run
```
ブラウザのプロキシをlocalhost:8080に設定し、各サイズのファイルにアクセスして観察。

## 11. 制約・前提

- 対応OS：Linux（Ubuntu 24.04を主対象）
- HTTPメソッドは大文字（GET、CONNECT等）として扱う
- HTTP/1.1のみサポート（hyper http1モジュール）。HTTP/2は対象外
- リクエストボディの最大サイズはhyperのデフォルト
- レスポンスボディは全部メモリに乗せる（最大100MB級を想定）
- 各NICのHTTPクライアントは接続タイムアウト10秒を設定する。GET全体の完了タイムアウトは設定しない
- CONNECTトンネルもTCP接続確立に10秒のタイムアウトを設定し、トンネル後の通信時間制限は設けない
- リトライしない。HEAD失敗等の判定不能ケースは単一NICフォールバック、Range分割中のchunk失敗は502とする
- マルチNIC前提（NIC数2固定）
- NIC IPとCONNECT宛先はIPv4のみ対応する
- 実験対象はローカルネットワーク内のHTTPサーバを想定し、ブラウザからのRangeヘッダ付きGETは来ない前提とする
- 並列リクエストはOSとTCPに任せる（NICの「使用中」管理はしない）

## 12. 非対象

以下は本MVPでは実装しない：

- HTTP/2、HTTP/3対応
- TLS終端MITM（HTTPSは素通し）
- HTTPSリソースのHEAD/Range分割ダウンロード
- URL指定で直接ダウンロードするCLI機能
- 圧縮転送されたレスポンスのRange分割
- IPv6対応
- 動的スケジューラ（実効スループット計測ベースの動的振り分け）
- リトライ・バックオフ
- 永続的なログファイル出力
- JSON形式の構造化ログ
- GUIまたはWebダッシュボード
- WebSocket対応
- 設定ファイルのホットリロード
- macOS、Windowsサポート
