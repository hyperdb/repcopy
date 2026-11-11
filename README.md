# repcopy

`repcopy`は2つのローカルリポジトリ間でファイルをコピーするためのツールです。git管理の対象外のファイルをコピーする目的で作成されました。

`rust`を使って開発されています。

## 使い方

exe化した後の実行イメージは以下の通りです。

```bash
repcopy <source directory> <target directory> --exclude="node_modules,.vscode"
```

- **source directory**: コピー元のディレクトリ
- **target directory**: コピー先のディレクトリ
- **--exclude**: 除外するディレクトリをカンマ区切りで指定（オプション）
- **--ignored-only**: gitで無視されているファイル（`.gitignore`、`.git/info/exclude`、グローバル設定）のみをコピー（オプション。gitのインストールと、コピー元がgitリポジトリ内にあることが必要）
- **-n, --dry-run**: 実際にはコピーせず、コピー対象のファイルを表示（オプション）

※コピー先にすでに存在するファイルはスキップします
※空のディレクトリはコピーしません
※除外するディレクトリは対象のルートにあるもの限定です
※ルート直下の`.git`ディレクトリは常に除外されます
※シンボリックリンクは警告を出してスキップします
※エラーが発生したファイルは報告して処理を続行し、終了コード1で終了します

## ビルド

```bash
cargo build --release
# => target/release/repcopy.exe
```
