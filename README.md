# kache-poc

[kache](https://github.com/kunobi-ninja/kache) が Rust の `target/` 肥大化を防げるかを macOS (APFS) 上で検証したリポジトリです。

## 検証したかったこと

- **アプリ間共有**: app1 と app2 が同じ crate の同じ version を使うとき、Mac にキャッシュは 1 つあり、両方の `target/` がそれを参照する形になっていること
- **worktree 間共有**: app1 の複数 worktree がそれぞれ同じ crate/version を使うとき、各 `target/` がキャッシュを参照する形になっていること

## 結論

**両方とも成立する。** kache は APFS の reflink (Copy-on-Write clone) を使い、`target/` に置かれる `.rlib`/`.rmeta` は store の blob と物理ブロックを共有する。

- 同じ crate/version は store に **1 部だけ** 存在する
- 各 `target/` からの参照は reflink なので `du` は独立したファイルに見えるが、実ディスクは共有される
- あるアプリの `target/` を消しても、store が持っている限り実ディスクはほとんど解放されない

詳しい数値と検証手順は [REPORT.html](./REPORT.html) を参照。

## 実験環境

- macOS Darwin 25.3.0 / APFS
- kache 0.12.0 (Homebrew)
- cargo 1.92.0

## 再現手順

### 1. kache のインストール

```sh
brew install kunobi-ninja/kunobi/kache
kache doctor
```

`kache doctor` の指示に従い `~/.cargo/config.toml` に `rustc-wrapper = "kache"` を設定。

このリポジトリの検証では、既存の sccache 設定を壊さないため、env で明示指定した。

```sh
export RUSTC_WRAPPER=kache
export CARGO_INCREMENTAL=0   # kache はインクリメンタルを無効化する
```

### 2. app1 / app2 のシナリオ

```sh
yes | kache purge                                     # store を空にする
(cd app1 && RUSTC_WRAPPER=kache CARGO_INCREMENTAL=0 cargo build)  # cold build
(cd app2 && RUSTC_WRAPPER=kache CARGO_INCREMENTAL=0 cargo build)  # 依存が同じなので cache hit
```

### 3. worktree のシナリオ

```sh
cd app1
git init && git add . && git commit -m init
git worktree add ../app1-wt-a HEAD
git worktree add ../app1-wt-b HEAD
(cd ../app1-wt-a && RUSTC_WRAPPER=kache CARGO_INCREMENTAL=0 cargo build)
(cd ../app1-wt-b && RUSTC_WRAPPER=kache CARGO_INCREMENTAL=0 cargo build)
```

### 4. 検証ポイント

同じ crate の `.rlib` が「同じ内容 / 異なる inode / link count=1」なら APFS reflink。

```sh
shasum -a 256 app1/target/debug/deps/libanyhow-*.rlib app2/target/debug/deps/libanyhow-*.rlib \
  ~/Library/Caches/kache/store/blobs/**/*
stat -f "inode=%i links=%l size=%z %N" <各ファイル>
```

決定的な確認は「target を消したときの実ディスク解放量」。論理サイズ 25 MB を消して 3–4 MB しか解放されなければ、残りは store と reflink 共有されている証拠。

```sh
BEFORE=$(df -k /Users | awk 'NR==2{print $4}')
rm -rf app2/target
sync
AFTER=$(df -k /Users | awk 'NR==2{print $4}')
echo "freed KB: $((AFTER - BEFORE))"
```

## 補足

- kache は bin crate (`app1`/`app2` 本体) や build script 出力はキャッシュしない (`cache_executables=false`)。target を消したときに解放される数 MB はこの分。
- reflink が使えるファイルシステムでのみ zero-copy。ext4 のようなファイルシステムでは hardlink または copy にフォールバックする。
- 別マシン間の共有は S3 or 共有 FS 経由の remote cache が必要。
