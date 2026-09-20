# Typst Equation Clipboard

Typst形式で数式を入力し、PowerPointへ貼り付けられるSVGおよびPNGとしてWindowsのクリップボードへコピーするデスクトップアプリです。

コピー形式は画面上の「コピー形式」から選択できます。SVGはAffinity Designer/Publisherなどのベクター用途、PNGは互換性を優先した貼り付けに適しています。

## 必要環境

- Windows 10/11
- [Typst](https://github.com/typst/typst) 0.14以降（`typst`をPATHへ追加）
- 開発時: Node.js、Rust、Microsoft C++ Build Tools、WebView2

## 開発

```powershell
npm install
npm run tauri dev
```

## テストとビルド

```powershell
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo build --release --manifest-path src-tauri/Cargo.toml
```
