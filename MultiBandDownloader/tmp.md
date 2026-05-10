# 分散ダウンロード高速化の検証計画

## 結論

ブラウザを内包したCLIツールを最初から作る必要はない。

まずは現在のHTTPプロキシに「分散モード」と「単一NICモード」を切り替える機能と、比較可能なCSVログ出力を追加するのが最小で確実。これにより、同じブラウザ、同じプロキシ、同じHTTPサーバ、同じ測定形式で、2NIC分散が単一NICより速いかを検証できる。

ブラウザ全体のページロード時間まで自動測定したくなった場合だけ、PlaywrightやSeleniumで外部ブラウザを起動する自動化を追加する。これは「ブラウザを内包する」のではなく、「既存ブラウザを制御する」形で十分。

## 現状の課題

現在のログでは、分散取得した1回のリクエストについて以下は分かる。

- 各NICが担当したbyte range
- 各NICの所要時間
- 分散時の合計所要時間
- 分散時のaggregate throughput

しかし、同じ条件で単一NIC取得した場合の時間が同時に記録されないため、「分散した結果、本当に速くなったか」は判断できない。

高速化の検証には、最低でも以下の比較が必要。

- 単一NICで同じファイルを取得した時間
- 2NIC分散で同じファイルを取得した時間
- 同条件で複数回測った平均・中央値・ばらつき
- ブラウザキャッシュやサーバキャッシュの影響を除いた測定

## 解決方法の候補

### 方法A: プロキシに比較モードを追加する

プロキシ設定またはCLI引数で取得方式を切り替える。

```toml
[experiment]
mode = "split" # split | single
csv_log = "results.csv"
```

`split`は現在の2NIC Range分割取得、`single`はNIC1だけで通常GETするモードにする。

同じURLを複数回取得し、`results.csv`に結果を保存する。

記録する項目の例:

```csv
timestamp,mode,url,size_bytes,status,total_ms,nic0_ms,nic1_ms,nic0_bytes,nic1_bytes,aggregate_mbps,error
```

利点:

- 現在の実装を活かせる
- 実装コストが低い
- ブラウザを使った実験と相性がよい
- プロキシ自身のオーバーヘッド込みで比較できる

欠点:

- ブラウザ操作は手動になる
- キャッシュ無効化や試行回数の管理は別途ルール化が必要

推奨度: 高

## 方法B: プロキシにベンチマーク用サブコマンドを追加する

ブラウザを使わず、CLIから指定URLを単一NICと2NIC分散の両方で取得して比較する。

例:

```bash
cargo run -- benchmark http://raspberrypi.local/test-100mb.bin --runs 10
```

出力例:

```text
URL: http://raspberrypi.local/test-100mb.bin
Runs: 10

single:
  median: 12.4s
  avg:    12.8s

split:
  median: 7.1s
  avg:    7.4s

speedup:
  median: 1.75x
```

利点:

- 自動で繰り返し測定できる
- 統計を出しやすい
- ブラウザキャッシュの影響を受けない

欠点:

- 「ブラウザで本当に速くなるか」ではなく、「HTTP取得部分が速くなるか」の検証になる
- 現在のプロキシ実装とは別の実行経路が増える

推奨度: 中

## 方法C: 外部ブラウザを自動操作する

Playwright、Selenium、Chrome DevTools Protocolなどで外部ブラウザを起動し、プロキシ設定を指定してページロード時間を測る。

例:

```bash
chromium --proxy-server=http://127.0.0.1:8080 --user-data-dir=/tmp/mnic-profile
```

測定対象:

- Navigation Timing APIの`loadEventEnd - startTime`
- Resource Timing APIの対象ファイルの`responseEnd - startTime`
- DevTools ProtocolのNetworkイベント

利点:

- ブラウザから見た実際の体感に近い
- ページ内に複数リソースがある場合も測定できる
- 手動操作を減らせる

欠点:

- 実装コストが高い
- ブラウザ依存の問題が増える
- 今回のMVPではHTTPSは分散対象外なので、一般Webサイトでは効果を測りにくい

推奨度: 低から中

## 方法D: 分散リクエストと同時に単一NICリクエストも裏で走らせる

1回のブラウザリクエストに対して、ブラウザへ返す分散取得とは別に、単一NIC取得も裏で実行して比較する。

これは一見公平に見えるが、推奨しない。

理由:

- 同じサーバへ余計なリクエストを投げるため、測定対象のネットワーク状態を変えてしまう
- サーバ負荷が増え、分散取得自体が遅くなる可能性がある
- キャッシュやTCP輻輳制御の影響で結果が解釈しにくい

推奨度: 低

## 推奨する進め方

### Step 1: プロキシに実験モードを追加する

`config.toml`に`[experiment]`を追加する。

```toml
[experiment]
mode = "split"
csv_log = "results.csv"
```

`mode`の候補:

- `split`: Range対応なら2NIC分散する
- `single`: Range対応でもNIC1だけで取得する

まずはこの2つで十分。

### Step 2: CSVログを追加する

ターミナル出力とは別に、機械処理しやすいCSVを残す。

最低限の列:

```csv
timestamp,mode,method,url,size_bytes,http_status,total_ms,nic0_ms,nic1_ms,nic0_bytes,nic1_bytes,error
```

`single`モードでは`nic1_ms`と`nic1_bytes`は空にする。

### Step 3: 測定ルールを固定する

公平な比較のため、以下を固定する。

- 同じRaspberry Pi上のHTTPサーバを使う
- 同じファイルサイズを使う
- ブラウザキャッシュを無効化する
- URLに`?run=1`などのcache-busting queryを付ける
- `split`と`single`を交互に測る
- 各サイズで最低10回ずつ測る
- 平均だけでなく中央値も見る

対象ファイル例:

- `1MB`
- `10MB`
- `50MB`
- `100MB`

### Step 4: 結果を集計する

CSVから以下を計算する。

- `single`の中央値
- `split`の中央値
- speedup = `single_median / split_median`
- 各モードのばらつき
- ファイルサイズ別の効果

判断基準の例:

```text
speedup > 1.2: 明確に速い
speedup 0.9-1.2: 誤差または条件依存
speedup < 0.9: 分散により遅くなっている
```

### Step 5: 必要ならブラウザ自動化を追加する

手動測定で効果が見えた後に、ページロード全体を測りたくなった場合だけブラウザ自動化を検討する。

この段階でも、ブラウザをCLIに内包する必要はない。CLIから外部ブラウザを起動し、プロキシ設定と測定URLを渡せばよい。

## 実験時の注意点

ブラウザキャッシュが有効だと、2回目以降の通信が発生せず、測定が無意味になる。

Raspberry Pi側のHTTPサーバがボトルネックになっている場合、2NICにしても速くならない。これは失敗ではなく、「サーバ側が支配的な条件では2NIC化の効果が出ない」という重要な観察結果になる。

Wi-FiやUSB NICを使う場合、2つのNICが実際には同じ物理帯域を共有している可能性がある。この場合も速度向上は限定的になる。

小さいファイルではHEAD、Rangeリクエスト2本、結合処理のオーバーヘッドにより、単一NICより遅くなる可能性が高い。

HTTPSは本MVPではCONNECTで素通しするため、一般的なWebサイトでは分散効果を測れない。検証対象はHTTPで配信されるローカル実験用ファイルに限定する。

## 次に実装すべき内容

優先順位は以下。

1. `config.toml`に`[experiment].mode`を追加する
2. `split`と`single`を切り替えられるようにする
3. CSVログ出力を追加する
4. `results.csv`を集計する簡単なスクリプトまたはREADME手順を書く
5. 必要になったら外部ブラウザ自動化を検討する
