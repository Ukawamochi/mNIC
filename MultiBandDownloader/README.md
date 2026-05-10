# mNIC-CLI

2つのNICを使ったTCP接続のラウンドロビン割り当てと、HTTP Range並列ダウンロードの効果を観察するための実験用HTTPプロキシです。

## セットアップ

プロジェクトルートに`config.toml`を作成します。

```bash
cp config.example.toml config.toml
```

その後、NICのIPアドレスを実環境に合わせて編集してください。

```toml
[[nics]]
ip = "192.168.1.10"

[[nics]]
ip = "192.168.1.11"

[proxy]
listen = "127.0.0.1:8080"
```

2つのNICアドレスは、ローカルのLinuxホストに割り当てられているIPv4アドレスである必要があります。

## 起動

```bash
cargo run
```

Range分割GETを有効にする場合だけ、明示的に`--range-split`を指定します。

```bash
cargo run -- --range-split
```

起動後、ブラウザのHTTPプロキシを`127.0.0.1:8080`に設定してください。

デフォルトではRange分割GETは無効です。HTTP GET、HTTPS CONNECT、その他HTTPメソッドは、ブラウザからプロキシへ届いたTCP接続ごとにNIC 0、NIC 1、NIC 0...の順で割り当てられます。

`--range-split`を指定した場合、HTTP GETではHEADリクエストで上流サーバの対応状況を確認します。上流サーバが`Accept-Ranges: bytes`に対応し、`Content-Length`が2バイト以上であれば、2つのNICを使って2つのRangeを並列取得し、結合した`200 OK`レスポンスをブラウザへ返します。

HTTPSは中身を解析しません。CONNECTリクエストはTLS終端せず、割り当てられたNIC経由でそのままトンネルします。

## 表示

実行中は、ターミナル上の同じ画面を1秒ごとに更新します。

- 直近10秒平均のNIC別TX/RX
- 起動後累計のNIC別TX/RX
- アクティブな接続
- 直近イベント

ここで表示するTX/RXは、このプロキシが扱ったpayload量です。TCP/IPヘッダ、Ethernetヘッダ、TLSレコードヘッダ、再送分などは含みません。

## 経路確認

送信元IPを固定しても、実際にどの物理NICから出るかはLinuxのルーティング設定に依存します。実験前に対象サーバへの経路を確認してください。

```bash

ip route get 192.168.24.1 from 192.168.24.17
ip route get 192.168.24.1 from 192.168.24.21

```

期待する状態は、それぞれ別の`dev <interface>`が表示されることです。
