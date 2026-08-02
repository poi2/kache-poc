# kache は Rust の `target/` 肥大化を防げるか

アプリ間・worktree 間の実ディスク共有を macOS (APFS) で実測した。

`kache 0.12.0` · `cargo 1.97.1` · `Darwin 25.3.0 (APFS)`

> **結論:** 両ケースで成立する。kache は APFS reflink を使い、同じ crate/version の `.rlib` を store に 1 部だけ持ち、各 `target/` の中身は store の blob と物理ブロックを共有する。26 MB の `target/deps` を消しても、実ディスクは 4.1 MB しか解放されなかった。

## ケース 1: app1 と app2 が同じ crate を使う

app1 と app2 は別ディレクトリの独立した Cargo プロジェクト。両者とも `serde = "1.0.219"` と `anyhow = "1.0.100"` に依存させた。

| 指標 | 値 | 備考 |
|---|---|---|
| app1 cold build | **2.92 s** | 初回、store は空 |
| app2 warm build | **1.03 s** | app1 と同じ依存 → 全部 cache hit |
| store サイズ | **25.4 MB** | app1/app2 で共有、追加コピーなし |

### reflink の指紋を確認

3 か所 (app1 / app2 / store) の `libanyhow-*.rlib` を比較。

| 場所 | SHA-256 | inode | link count |
|---|---|---:|---:|
| `app1/target/…/libanyhow-*.rlib` | `5cddf8de…` | 77809861 | 1 |
| `app2/target/…/libanyhow-*.rlib` | `5cddf8de…` | 77810279 | 1 |
| `store/blobs/72/7245120f…` | `5cddf8de…` | 77809869 | 1 |

3 つとも内容は同一だが inode は別。link count = 1 なので hardlink ではない。「異なる inode + 同一内容 + link=1」は APFS reflink (CoW clone) の指紋である。

### 物理ディスク共有の実測

app2 の `target/` を丸ごと削除して、実際の空き容量が何 KB 増えるかを見た。

| 指標 | 値 |
|---|---:|
| 削除した `target/` の論理サイズ (`du`) | 26 MB (~26,624 KB) |
| 削除後に `df` で観測された解放量 | **4,088 KB** |
| 差 = store と reflink 共有されていた分 | ~22 MB |

4.1 MB の解放は、kache がキャッシュしない `app2` 本体の実行バイナリ、build script の出力、`.d`/fingerprint ファイル分だと考えられる (`cache_executables=false` がデフォルト)。

## ケース 2: app1 の worktree 2 個が同じ crate を使う

app1 を git 化し、`git worktree add` で 2 つの worktree (`app1-wt-a`, `app1-wt-b`) を作成。それぞれで `cargo build`。

| 指標 | 値 | 備考 |
|---|---|---|
| `app1-wt-a` build | **0.93 s** | store には既に blob あり |
| `app1-wt-b` build | **0.87 s** | 同上 |
| store 追加量 | **0 MB** | 既存 blob をそのまま参照 |

### 両 worktree の `target/` を消したときの実解放

| 指標 | 値 |
|---|---:|
| 2 つの `target/deps` の論理サイズ合計 | ~52 MB |
| 両方削除後に `df` で観測された解放量 | **8,124 KB** |
| 差 = store と reflink 共有されていた分 | ~44 MB |

worktree を増やしても、cargo が作る `target/` のうち rustc アーティファクト部分は store と物理ブロックを共有するので、実ディスクはほぼ増えない。

## 参照関係のイメージ

```
# どの target/ の .rlib も、実データは 1 つの store blob を指す

app1/target/…/libanyhow-*.rlib   ─┐
app2/target/…/libanyhow-*.rlib   ─┼──▶   ~/Library/Caches/kache/store/blobs/72/7245120f…
app1-wt-a/target/…/libanyhow…    ─┤       (APFS 上、物理ブロックは 1 部だけ)
app1-wt-b/target/…/libanyhow…    ─┘
```

各 `.rlib` は独立した inode を持ち、`ls` や `du` からは普通のファイルに見える。APFS が blocks を CoW 共有しているだけなので、書き換えたら自動的にブロックが分岐する。安全に「独立したファイル」として扱える。

## kache 自身のレポート

`kache report` の出力から抜粋。app1/app2 の連続ビルド直後。

```
50.0% hit rate — 8/16 cacheable crates cached, 8 compiled
Storage:
  Restored: 25.4 MB (100.0% zero-copy, 0 B copied)
  Store: 25.4 MB logical -> 25.4 MB blobs (0 B dedup saved)
```

**100.0% zero-copy** = 復元は 1 バイトもコピーせずすべて reflink で済んだ、という意味。

## 実運用への含意

- ローカルに複数の Cargo プロジェクトがあり、共通依存が多いほど kache の効果は大きい。同じ crate/version は store 上に 1 部だけ。
- PR 用に worktree を並べる運用でも、`target/` は膨らみにくい。
- bin crate 本体、build script 出力、`.d`/fingerprint はキャッシュされないので、`target/` の全部がゼロにはならない (実測で 3–4 MB / プロジェクト程度は残った)。
- zero-copy 復元は APFS / btrfs / XFS-with-reflink でのみ効く。ext4 のような FS では hardlink または copy にフォールバック ([README.md](./README.md) 参照)。
- マシン間の共有は S3 or 共有 FS 経由の remote cache が必要。今回はローカルのみ検証した。

## 再現手順の要点

```sh
# kache 導入
brew install kunobi-ninja/kunobi/kache
yes | kache purge

# 検証環境
export RUSTC_WRAPPER=kache
export CARGO_INCREMENTAL=0   # kache がインクリメンタルを止める

# ケース 1
(cd app1 && cargo build)   # cold
(cd app2 && cargo build)   # warm — 同じ依存

# 実ディスク共有の確認
BEFORE=$(df -k /Users | awk 'NR==2{print $4}')
rm -rf app2/target
sync
AFTER=$(df -k /Users | awk 'NR==2{print $4}')
echo "freed KB: $((AFTER - BEFORE))"   # 論理 26 MB に対し 4–5 MB のはず
```

詳しくは [README.md](./README.md) を参照。

---

検証: 2026-08-02 · kache [kunobi-ninja/kache](https://github.com/kunobi-ninja/kache)
