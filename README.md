# pic_yomi

在漫画、网页图片、甚至任意 App 的画面上查日语生词：识别图中文字 → 日日词典释义（新明解等）+ 中文解释 → 一键加入生词本 → 用 FSRS 记忆曲线背 → 手机离线复习、自动同步。

不再依赖 Yomitan 和 Anki。所有数据都在你自己的 Mac 上。

> v1.0 是对早期版本（mokuro + Yomitan 胶水脚本）的完全重写。[English summary ↓](#english)

---

## 它由什么组成

```
┌─────────────── Mac：pic_yomi.app（Rust + GPUI）────────────────┐
│  复习 / 词单 / 查词 / 设置 界面                                  │
│  ⌥⌘J 全局屏幕查词：截取鼠标附近 → OCR → 浮窗查词                   │
│  OCR：RT-DETR 文字检测 + charspot 逐字识别（ONNX Runtime，CPU）    │
│  词典：导入 Yomitan 词典包（SQLite）+ 词形还原 + lindera 注音       │
│  中文解释：任意 OpenAI 兼容接口 / Gemini（按句缓存）                 │
│  FSRS-7 排程（官方 fsrs crate）                                  │
│  本地 API 127.0.0.1:7331 ←── 浏览器扩展（Chrome / Zen / Firefox） │
└───────────────┬──────────────────────────────────────────────┘
                │ tailscale serve（HTTPS，只有你的设备能访问）
        手机 PWA（离线复习，联网自动同步；FSRS 用同一份 Rust 代码编译成 WebAssembly）
```

| 目录 | 内容 |
|---|---|
| `crates/app` | 桌面程序（GPUI / gpui-kit） |
| `crates/core` | 词典、查词、OCR、生词卡、同步、LLM、本地 API |
| `crates/srs` | FSRS-7 排程，同时编译成 WebAssembly 给手机用 |
| `extension` | 浏览器扩展（网页图片 Shift 悬停识别、网页文字查词） |
| `web` | 手机复习 PWA（Preact） |

## OCR：为什么换掉 mokuro

在 [JMangaBench_Read](https://github.com/muscgab/JMangaBench_Read)（2026-10，7,789 个漫画文字框）的 150 个样本上，在这台 M4 上实测：

| 识别模型 | 字符错误率 | 整句全对 | 速度 |
|---|---|---|---|
| **charspot-small（现在）** | **1.49%** | **92.7%** | **19 ms/框**（CPU） |
| manga-ocr（旧版） | 6.14% | 73.3% | 244 ms/框（GPU） |

文字检测用 RT-DETR（ogkalu/comic-text-and-bubble-detector），整页检测 + 识别约 0.3 秒，不需要 PyTorch。charspot 还会给出**每个字的位置**，所以鼠标指到哪个字就从哪个字开始查。

## 安装

需要：macOS（Apple Silicon）、[Rust](https://rustup.rs)、Node.js。不需要 Xcode（只要 Command Line Tools）。

```bash
git clone https://github.com/peinniku/manga-ocr-yomitan.git pic_yomi
cd pic_yomi
scripts/bundle-macos.sh --install      # 构建并安装到 /Applications/pic_yomi.app
```

第一次打开后，在「设置」里：

1. **OCR 模型 → 下载模型**（约 55 MB，带校验）。
2. **词典 → 导入词典…**：选你的 Yomitan 词典包 `.zip`（例如新明解、大辞林、JPDB 频率、Jitendex）。词典只存在本机，**不会进 Git 仓库**（它们有版权）。
3. **中文解释**：填接口地址、模型和 API Key，点「测试」。例如 DeepSeek：`https://api.deepseek.com` + `deepseek-flash`；Gemini：`https://generativelanguage.googleapis.com/v1beta` + `gemini-3.5-flash-lite`。不填也能用，只是没有中文解释。

### 屏幕查词（任意 App）

和 Yomitan 一样：**按住 Shift，把鼠标移到日语上**，弹窗跟着鼠标查词，查到的词会在屏幕上标出来；滑到别的字就换词，点别处关闭。按住 Shift 时只截一次图、识别一次，之后滑动只是查已识别的文字，所以跟手。浏览器里默认交给扩展（它直接读网页文字），可在「设置 → 屏幕查词」里打开。

**⌥⌘J** 是另一种用法：识别鼠标附近的文字并打开一个可操作的查词窗口。

横排文字（推文、网页、App 界面）用 macOS 自带的 Vision 识别，漫画竖排文字用上面的 charspot。第一次会请求「屏幕录制」权限（系统设置 → 隐私与安全性 → 录屏与系统录音 → 允许 pic_yomi），截图只在本机处理。

自己打包时建议先运行一次 `scripts/make-signing-cert.sh`：它在钥匙串里建一个本机签名证书，之后每次重新打包签名身份不变，屏幕录制权限不用重新授权。

### 浏览器扩展

```bash
node extension/build.mjs
```

- **Chrome**：`chrome://extensions` → 开启开发者模式 → 加载已解压的扩展程序 → 选 `extension/dist/chrome`。
- **Zen**（长期安装）：Zen 允许关闭扩展签名检查。在 `about:config` 把 `xpinstall.signatures.required` 设为 `false`，然后 `about:addons` → 齿轮 → 从文件安装附加组件 → 选 `extension/dist/picyomi-firefox.xpi`。
- **Firefox**：正式版必须签名。临时用：`about:debugging` → 此 Firefox → 临时载入附加组件 → 选 `extension/dist/firefox/manifest.json`（重启后失效）；长期用需要自己的 AMO 账号免费签名：`npx web-ext sign --channel=unlisted --source-dir extension/dist/firefox --api-key=… --api-secret=…`。

用法：鼠标放在图片上按住 **Shift** → 识别；再按住 Shift 指向图中或网页上的日语 → 弹出释义 → 「＋ 加入词单」（会保存气泡截图）。

### 手机

1. Mac 和 iPhone 都登录 [Tailscale](https://tailscale.com)，在 Tailscale 后台开启 HTTPS 证书（MagicDNS → HTTPS Certificates）。
2. 在 Mac 终端运行一次：`tailscale serve --bg 7331`（只在你的 tailnet 内可见，不暴露到公网）。
3. 打开 pic_yomi →「设置 → 手机同步」，用 iPhone 相机扫二维码，在 Safari 中打开 →「分享 → 添加到主屏幕」。

手机上的复习记录先存在本机，离线也能背；联网（能连到 Mac）时自动同步。两边对同一张卡的复习会合并，并用同一份 FSRS 代码重算排程，不会冲突。

### 发音

查词弹窗、复习卡背面、词单详情里都有 🔊：单词和整句都能听。

- **单词**：优先用真人录音（JapanesePod101，没有再找 Lingua Libre），和 Yomitan 默认的来源一样；两边都没有时用合成语音。
- **整句和没有录音的词**：Azure 神经语音（「设置 → 发音」填 key 和区域，可选 8 种声音）。同形异读的词（上手＝じょうず/うわて）按词典读音念，其他词让声音按自己的词库读，声调更自然。没填 key 时用 macOS 自带的 Kyoko。
- 复习翻面时自动念一遍单词（可关），按 R 再听。手机端会提前下载今天要复习的发音，离线也能听。
- 每个发音只取一次，缓存在数据目录的 `audio/` 里。

## 数据与隐私

- 数据目录：`~/Library/Application Support/pic_yomi/`（SQLite 数据库、生词卡截图、OCR 模型）。
- 本地 API 只监听 `127.0.0.1`；除健康检查外，所有接口都要配对令牌。网页无法读取令牌，经 Tailscale 代理的请求只能通过二维码配对。
- 只有「中文解释」会把**单词和所在句子**发给你配置的 LLM 服务。
- 发音：单词会发给 JapanesePod101 和 Wikimedia Commons 查录音；合成语音会把单词或句子发给 Azure（填了 key 时）。

## 模型许可

- 检测：ogkalu/comic-text-and-bubble-detector，Apache-2.0。
- 识别：muscgab/manga-ocr-charspot-small，**CC BY-NC-SA 4.0（仅限非商业使用）**。模型在首次使用时下载，不随仓库分发。

## 开发

```bash
cargo test --workspace                      # Rust 单元测试（词形还原、查词、FSRS、同步合并…）
cd web && npm install && npm test           # 手机端：wasm 排程与 Rust 原生结果一致性
cargo run -p picyomi                        # 运行桌面程序（调试版）
cargo run -p picyomi-core --bin picyomi-cli -- lookup 食べなかった
cargo run -p picyomi-core --bin picyomi-cli -- ocr page.jpg
```

CLI 还有 `import-dict`、`dicts`、`serve`、`set-llm`（Key 从环境变量 `LLM_API_KEY` 读取）、`explain`。

### 从 Anki 迁移

```bash
cargo run -p picyomi-core --bin picyomi-cli -- import-anki ~/Library/Application\ Support/Anki2/<账户>/collection.anki2 Yomitan
```

导入带 `Expression` / `Reading` 字段的笔记（Yomitan 制卡的格式），最后一个参数是牌组名（省略则导入所有牌组）。释义优先用本机导入的词典，查不到时保留 Anki 里的文字；Anki 的复习记录会一起导入，FSRS 据此接着排程。Anki 开着也没关系（先在内存里拷一份再读，不改动 Anki）；已在词单里的词会跳过，可以重复运行。

## 致谢

[charspot / JMangaBench_Read](https://github.com/muscgab)、[ogkalu 的检测模型](https://huggingface.co/ogkalu/comic-text-and-bubble-detector)、[fsrs-rs](https://github.com/open-spaced-repetition/fsrs-rs)、[GPUI](https://www.gpui.rs) / [gpui-kit](https://github.com/longbridge/gpui-kit)、[ONNX Runtime / ort](https://ort.pyke.io)、[lindera](https://github.com/lindera/lindera)、Yomitan 词典格式。早期版本基于 [mokuro](https://github.com/kha-white/mokuro) 和 [manga-ocr](https://github.com/kha-white/manga-ocr)。

MIT License — 见 [LICENSE](LICENSE)。

---

## English

pic_yomi looks up Japanese words in manga, web images and any on-screen text, then turns them into flashcards scheduled with FSRS-7, with offline review on your phone.

- **Desktop app** (Rust, GPUI): review, word list, lookup, settings; global hotkey ⌥⌘J OCRs the screen around the cursor.
- **OCR**: RT-DETR text detection + charspot per-character recognition via ONNX Runtime (1.49% CER vs 6.14% for manga-ocr on a JMangaBench sample, ~0.3 s per page on an M4 CPU).
- **Dictionaries**: import any Yomitan dictionary zip; rule-based deinflection plus tokenizer fallback; optional Chinese explanations from any OpenAI-compatible or Gemini endpoint.
- **Browser extension**: Shift-hover images to OCR, Shift-hover text to look up, one click to save the word with a crop of the speech bubble.
- **Phone**: a PWA reached over `tailscale serve`; reviews work offline and sync later. Scheduling runs the same Rust code compiled to WebAssembly, so devices always agree.

Build with `scripts/bundle-macos.sh --install`. The recognition model is CC BY-NC-SA 4.0 (non-commercial) and is downloaded on first use.
