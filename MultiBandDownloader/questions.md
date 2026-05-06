# Codexと私のQA Session

## Q1 ーーーーーーーーー

`specification.md`の5.4「その他メソッド」に、`paradl`、`Target: https://example.com/file.zip`、進捗バー、出力ファイル名、SHA-256など、HTTPプロキシではなく直接ダウンローダCLIのような出力例が混ざっています。

このプロジェクトは「ブラウザ用HTTPプロキシ」に限定しますか？ それとも「URLを指定して直接ダウンロードするCLI」機能も含めますか？

回答: このプロジェクトはブラウザ用httpプロキシに限定してください。CLI機能は不要です。

## Q2 ーーーーーーーーー

仕様ではHTTPSはCONNECTで素通しし、実験対象外とされています。一方で、7節では「HTTPSのHEADリクエストには将来的に必要」とあり、5.4にも`https://example.com/file.zip`が出ています。

MVPでは、HTTPSリソースに対するHEAD/Range分割ダウンロードは実装しない、という理解でよいですか？

回答:はい。古い情報が混ざっていたようです。HTTPSでAccept-Rangeによる分割をしようとすると、必然的にTLSを終端させる必要があり、実装に時間がかかる、実験に暗号化は必要ないの2つの理由から今回はhttpsは完全にスルーしてください。古い情報が残っていた部分は修正してください。

## Q3 ーーーーーーーーー

ブラウザ自身が`Range`ヘッダ付きGETを送るケースがあります。現仕様の4.3.5は「ブラウザはRangeを要求していないため200 OKで返す」としていますが、実際には動画・PDF・再開ダウンロード等でRange要求が来る可能性があります。

クライアントから`Range`ヘッダが来た場合は、Range分割の対象外としてNIC1でそのまま転送しますか？ それともクライアントRangeに対しても2NIC分割を試みますか？

回答:今回はテストのためにローカルネットワーク内のラズパイで動いているWebサーバーへアクセスします。なのでクライアントからRangeヘッダーつきGETが来ることは絶対ありません。なので無視して大丈夫です。

## Q4 ーーーーーーーーー

GET時にブラウザから受け取ったヘッダを、HEADリクエストおよび実際のGET/Range GETへどこまで転送するかが未定義です。特に`Cookie`、`User-Agent`、`Accept`、`Accept-Encoding`、`Authorization`、`Referer`は挙動に影響します。

MVPでは、ブラウザからの通常ヘッダを可能な範囲で転送し、hop-by-hopヘッダ（`Connection`、`Proxy-Connection`、`Transfer-Encoding`等）だけ除外する方針でよいですか？

回答:はい。その方針で実装してください。

一つずつ答える。

---

## Q5: Content-Encoding

**回答**：はい、Range分割対象のHEAD/GETでは`Accept-Encoding: identity`を送って非圧縮を要求する。

理由：圧縮されているとRangeのバイト位置が圧縮後のバイト列を指すため、分割位置の計算が混乱する。MVPでは実験条件を単純化する目的で非圧縮に揃える。これは応募書類でも「圧縮転送への対応は将来課題」として正直に書ける。

---

## Q6: HEAD失敗時

**回答**：NIC1の通常GETにフォールバックする。

理由：HEAD失敗で全部502にすると、ブラウジングが頻繁に壊れて実験そのものができなくなる。HEAD失敗はターミナルに記録した上で、NIC1単独GETで処理を続行する。出力例：

```
[GET]  http://example.com/api/data
       HEAD failed (405), falling back to single NIC
       NIC 192.168.1.10: full transfer in 0.12s
```

---

## Q7: Content-Lengthがない場合

**回答**：Range分割を諦めてNIC1通常GETにフォールバック。

理由：Q6と同じ。動的コンテンツやchunkedはWebでは普通なので、ここで502を返すと実用にならない。Range分割の対象外として記録に残し、通常GETで処理する。

---

## Q8: size == 0

**回答**：ゼロバイトレスポンスはRange分割せず、NIC1で通常GETするか、HEADの情報を元に空の200 OKを返す。

実装はシンプルに：`size < 2`の場合（0または1バイト）は分割しない条件を入れる。出力には`Size: 0 B, no split`等と表示。

---

## Q9: CONNECT・IP family

**回答**：IPv4のみ対応で問題ない。設定NICがIPv4前提なので、宛先もIPv4で揃える。

実装：

1. `req.uri().authority()`からホスト・ポート取得
2. `tokio::net::lookup_host()`でDNS解決
3. 結果からIPv4アドレスを選ぶ
4. IPv4が見つからなければエラー（CONNECTを失敗させる）

IPv6対応は将来課題。

---

## Q10: NIC IP型

**回答**：`Ipv4Addr`に限定する。

理由：MVPの実験条件はIPv4で統一されている。型を`Ipv4Addr`に絞ることで、検証が型レベルで強制される。`local_address`に渡すときは`IpAddr::V4(ip)`に変換する。

---

## Q11: タイムアウト

**回答**：「接続タイムアウト10秒」「全体タイムアウトなし」とする。

100MB級ファイルが10Mbps回線で完了するには80秒以上かかるため、全体タイムアウト10秒では実験にならない。一方、接続が確立できないケースは10秒で諦めるべき。

reqwestでの実装：

```rust
reqwest::Client::builder()
    .local_address(ip)
    .connect_timeout(Duration::from_secs(10))  // 接続のみ
    // .timeout は設定しない
    .build()
```

CONNECTトンネルも同様に、TCP接続確立に10秒のタイムアウトをかける。トンネル後の通信時間制限はなし。

---

## Q12: Range分割中の片方失敗

**回答**：MVPでは502で失敗させる。

理由：

- 通信条件は固定（仕様2節）であり、片方失敗は環境の異常を意味する
- フォールバック実装は複雑（既に半分受信したデータを破棄するか活用するか等）
- 実験用ツールとして、異常状態を観察できることに価値がある

ターミナル出力で原因を明示：

```
[GET]  http://raspberrypi.local/test.bin
       Size: 10.00 MB, Range: yes
       NIC 192.168.1.10: completed in 5.2s
       NIC 192.168.1.11: ERROR: connection refused
       FAILED: returning 502
```

---

## Q13: レスポンスヘッダ

**回答**：はい、hop-by-hopヘッダを除外し、`Content-Length`を再計算する。

除外すべきヘッダ：

- `Transfer-Encoding`
- `Connection`
- `Proxy-Connection`
- `Keep-Alive`
- `Upgrade`
- `TE`
- `Trailer`
- `Content-Range`（206→200に変えるため）

実装はホワイトリスト方式の方が安全：必要なヘッダ（`Content-Type`、`Content-Encoding`、`ETag`、`Last-Modified`、`Cache-Control`等）だけ転送し、それ以外は捨てる。`Content-Length`は連結後のサイズで再設定する。

---

## Q14: エラー型

**回答**：`anyhow`中心で簡潔に実装する。

理由：

- MVPの規模では専用エラー型は過剰
- 仕様6.2に書いた`thiserror`の例は誤り（依存クレートに含めていない）
- エラーの分岐処理がほぼないため、`anyhow::Result`で十分

仕様6.2の`http.rs`の関数シグネチャは以下に修正：

```rust
pub async fn head(client: &reqwest::Client, url: &str) -> anyhow::Result<ServerInfo>;
pub async fn get_range(...) -> anyhow::Result<(Vec<u8>, Duration)>;
pub async fn get_full(...) -> anyhow::Result<(Vec<u8>, Duration, HeaderMap)>;
```

依存クレートから`thiserror`は外す（元から入れていなかった）。`anyhow::Context`を使って`.context("HEAD failed for {url}")`等で文脈を付ける。
