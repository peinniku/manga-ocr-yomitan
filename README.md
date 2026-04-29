# manga-ocr-yomitan

OCR Japanese text inside any web image and feed it to [Yomitan](https://yomitan.wiki/) as if it were normal page text — so the same hover-and-add-to-Anki flow you use on text pages just works on manga, fan art, screenshots, and other in-image text.

> [中文文档 ↓](#中文文档)

---

## How it works

1. A small local Python server runs [`mokuro`](https://github.com/kha-white/mokuro) (text detection + manga-OCR recognition). Given an image it returns the bounding box and recognized text of every line, plus whether each block is vertical or horizontal.
2. A userscript watches `Shift`-hover on `<img>` elements. On hover it sends the image to the local server, then injects invisible, correctly-positioned text spans on top of the image — using the OCR result, with proper `writing-mode: vertical-rl` for vertical bubbles.
3. Yomitan sees these spans like any other Japanese text on a webpage. Hover to look up, click `+` to add to Anki — same flow you already have. The recognized line becomes the "example sentence" captured on your Anki card.

The first hover on each image takes 1–2 seconds for OCR; results are cached in memory, so repeat hovers on the same image are instant.

## What you need

These steps install everything from scratch. If you already have some of them, skip ahead.

- **Python + conda** for the OCR server
- **A modern browser** (Firefox / Zen / Chrome / Edge / Brave — anything that supports userscript managers)
- **A userscript manager** ([Violentmonkey](https://violentmonkey.github.io/) — recommended, open source — or Tampermonkey)
- **[Yomitan](https://yomitan.wiki/)** with at least one Japanese dictionary loaded; optionally [AnkiConnect](https://foosoft.net/projects/anki-connect/) configured if you want one-click Anki cards

Hardware: Apple Silicon Macs use MPS automatically. CUDA is used if available. CPU-only also works, just a bit slower per image.

## Install

### 1. Install Miniconda *(skip if you already have `conda`)*

macOS (Apple Silicon):

```bash
curl -LO https://repo.anaconda.com/miniconda/Miniconda3-latest-MacOSX-arm64.sh
bash Miniconda3-latest-MacOSX-arm64.sh
```

For Intel Mac / Linux / Windows, see the official [Miniconda installers](https://www.anaconda.com/docs/getting-started/miniconda/install).

After install, restart your terminal and verify: `conda --version`.

### 2. Clone this repo

```bash
git clone https://github.com/peinniku/manga-ocr-yomitan.git
cd manga-ocr-yomitan
```

### 3. Create the environment and install dependencies

```bash
conda create -n manga-ocr-yomitan python=3.11 -y
conda activate manga-ocr-yomitan
pip install -r requirements.txt
```

This pulls in `mokuro`, which brings `manga-ocr`, `comic-text-detector`, and PyTorch. Total disk footprint is roughly **2–3 GB** — PyTorch is most of it.

### 4. Start the OCR server

```bash
uvicorn ocr_server:app --host 127.0.0.1 --port 7331
```

**The first run downloads the OCR models (~600 MB)** to `~/.cache/`. After that, startup takes a few seconds. Keep this terminal open whenever you want OCR to work — when you close it, the script falls back to "no OCR available".

Quick check from another terminal:

```bash
curl http://127.0.0.1:7331/health
# {"ok":true,"loaded":false}
```

### 5. Install a userscript manager

[Violentmonkey](https://violentmonkey.github.io/) (recommended) or Tampermonkey, from your browser's extension store.

### 6. Install Yomitan *(skip if you already use it)*

Follow the [Yomitan setup guide](https://yomitan.wiki/setup/). Minimum:

- Yomitan extension installed
- One JMDict / 新明解 / etc. dictionary imported
- *(For Anki cards)* AnkiConnect installed in Anki, plus a Yomitan card template configured

This project doesn't change anything in Yomitan — it just makes images "look like text" to it.

### 7. Install this userscript

Click the **[Install userscript](https://github.com/peinniku/manga-ocr-yomitan/raw/main/manga-ocr-yomitan.user.js)** link (or open `manga-ocr-yomitan.user.js` from your local clone). Your userscript manager will prompt to install — confirm.

The first time it tries to fetch an image, your manager will ask for permission to connect to `*` and `127.0.0.1`. Allow both.

### 8. Confirm Yomitan's modifier key

Yomitan's default popup trigger is `Shift`, which is what this script also listens for. If you've changed it in Yomitan, edit the `keydown`/`mouseover` handlers in the userscript to match.

## Usage

1. Open any page with Japanese text inside images — manga sites, Pixiv, X/Twitter, screenshots embedded in articles.
2. Hover an image and **hold `Shift`** for 1–2 seconds. A status indicator appears at the bottom-right (`OCR…` → `OCR ✓ (N blocks)`).
3. Now `Shift`-hover the actual Japanese text — Yomitan pops up exactly like on any text page.
4. Click `+` in the Yomitan popup to add to Anki. The captured sentence is the OCR'd line from that bubble.

Each image is OCR'd once and cached in memory; later hovers on the same image are instant.

### Debug overlay

A small eye icon (👁) sits at the bottom-left of the page. Click it to toggle visible OCR boxes:

- Red border = detected block
- Green border = each line within the block
- Semi-transparent red glyphs = recognized text

Useful for verifying alignment when trying the script on a new site.

## Limitations & known issues

- **Only `<img>` elements**. CSS `background-image` and `<canvas>` aren't handled.
- **Skips images smaller than 200×200 px** (avoids OCR'ing avatars/icons). Adjust `MIN_DIM` in the userscript.
- **First OCR per image takes 1–2 seconds**. Hold `Shift` long enough.
- **OCR quality depends on the image**. Clean dialog bubbles are very accurate; hand-drawn sound effects, heavily stylized fonts, and very small text often miss.
- **Strict CSP sites** (e.g. some pages on x.com) require `GM_addStyle` to apply our CSS. The script already uses it; if your userscript manager doesn't grant that permission, styles won't apply and you'll see misaligned spans.
- **OCR server is local-only and unauthenticated**. It listens on `127.0.0.1` and decodes images you send to it; don't expose it on a public interface.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `OCR result: 0 blocks` in console | mokuro detected no text. Try a clearer / larger version of the image. |
| `network error` in console | OCR server isn't running, or you're on a different port than `7331`. |
| `OCR ✓` shows but Yomitan doesn't pop up | Yomitan disabled on that domain, or your modifier key differs from `Shift`. |
| Overlay is rendered (debug shows boxes) but Yomitan still doesn't trigger | Site is blocking pointer events on the image. Open an issue with the URL. |
| Gray rectangle covering the image after hovering | A previous version had this from text selection. Update to v0.2.0+. |

## Acknowledgments

This project is glue code on top of much harder work by others:

- [mokuro](https://github.com/kha-white/mokuro) — detection + recognition pipeline + the original "OCR overlay for manga HTML" idea
- [manga-ocr](https://github.com/kha-white/manga-ocr) — the recognition model
- [comic-text-detector](https://github.com/dmMaze/comic-text-detector) — the detection model
- [Yomitan](https://yomitan.wiki/) — the dictionary popup that does all the actual word-lookup work

## License

MIT — see [LICENSE](LICENSE).

---

## 中文文档

在网页图片里 OCR 日文，把识别结果以隐形文字的形式叠在图上，让 [Yomitan](https://yomitan.wiki/) 像处理普通网页文字一样查词、加 Anki。专门解决看漫画/二创/截图时遇到生词无法被 Yomitan 拾取的问题。

### 工作原理

1. 本地一个小 Python 服务跑 [`mokuro`](https://github.com/kha-white/mokuro)（文字检测 + manga-OCR 识别）。给一张图，返回每行的位置坐标、识别结果、以及是否竖排。
2. 油猴脚本监听 `Shift` 悬停 `<img>` 元素。悬停时把图发给本地服务，拿到 OCR 结果后在图上注入透明的、定位准确的文本 span，竖排框自动加 `writing-mode: vertical-rl`。
3. Yomitan 把这些 span 当作普通日文：悬停查词、点 `+` 加 Anki，跟你平时网页上的流程完全一样。Anki 卡片上抓到的"例句"就是那个气泡的 OCR 文本。

每张图首次 OCR 需要 1–2 秒，识别结果会缓存在内存里，之后再悬停同一张图是瞬时的。

### 你需要

下面的步骤会引导你从零装齐这些。如果有些已经装了，跳过就行。

- **Python + conda**——给 OCR 服务用
- **现代浏览器**（Firefox / Zen / Chrome / Edge / Brave，任何支持油猴扩展的都行）
- **油猴管理器**（[Violentmonkey](https://violentmonkey.github.io/) 推荐，开源；或 Tampermonkey）
- **[Yomitan](https://yomitan.wiki/)**——至少导入了一本日语词典；如果想加 Anki，还要装好 [AnkiConnect](https://foosoft.net/projects/anki-connect/) 和卡片模板

硬件：Apple Silicon 自动用 MPS；有 CUDA 用 CUDA；纯 CPU 也能跑，只是每张图首次 OCR 慢一点。

### 安装

#### 1. 装 Miniconda *（已经有 `conda` 跳过）*

macOS（Apple Silicon）：

```bash
curl -LO https://repo.anaconda.com/miniconda/Miniconda3-latest-MacOSX-arm64.sh
bash Miniconda3-latest-MacOSX-arm64.sh
```

Intel Mac / Linux / Windows 见 [Miniconda 官方安装包](https://www.anaconda.com/docs/getting-started/miniconda/install)。

装完重开终端，验证：`conda --version`。

#### 2. 克隆本仓库

```bash
git clone https://github.com/peinniku/manga-ocr-yomitan.git
cd manga-ocr-yomitan
```

#### 3. 建环境装依赖

```bash
conda create -n manga-ocr-yomitan python=3.11 -y
conda activate manga-ocr-yomitan
pip install -r requirements.txt
```

会拉 `mokuro` + `manga-ocr` + `comic-text-detector` + PyTorch，总大小约 **2–3 GB**——PyTorch 占大头。

#### 4. 启动 OCR 服务

```bash
uvicorn ocr_server:app --host 127.0.0.1 --port 7331
```

**首次启动会下载 OCR 模型（约 600 MB）** 到 `~/.cache/`。之后启动几秒。要让 OCR 工作，这个终端要一直开着——关掉之后脚本就回到"没 OCR 可用"的状态。

另开一个终端验证：

```bash
curl http://127.0.0.1:7331/health
# {"ok":true,"loaded":false}
```

#### 5. 安装油猴管理器

在你浏览器的扩展商店装 [Violentmonkey](https://violentmonkey.github.io/)（推荐）或 Tampermonkey。

#### 6. 安装 Yomitan *（已经在用跳过）*

按 [Yomitan 安装指南](https://yomitan.wiki/setup/) 来。最低要求：

- Yomitan 扩展已装
- 至少导入了一本词典（JMDict / 新明解 等）
- *（要加 Anki 的话）* Anki 装好 AnkiConnect，Yomitan 里配好卡片模板

本项目不改 Yomitan 任何设置——只是让图片"对 Yomitan 来说像文字"。

#### 7. 安装本油猴脚本

点 **[安装脚本](https://github.com/peinniku/manga-ocr-yomitan/raw/main/manga-ocr-yomitan.user.js)** 链接（或打开本地 clone 里的 `manga-ocr-yomitan.user.js`）。油猴会弹安装确认，同意。

第一次悬停图片要拉图时，油猴会再问"是否允许连 `*` 和 `127.0.0.1`"，全部允许。

#### 8. 确认 Yomitan 修饰键

Yomitan 默认弹窗触发键是 `Shift`，也是这个脚本监听的。如果你改成了别的，需要改脚本里的 `keydown`/`mouseover` 处理逻辑。

### 使用

1. 打开任意有日文图片的页面——漫画站、Pixiv、X/Twitter、文章里嵌的截图都行。
2. 悬停图片，**按住 `Shift`** 1–2 秒。右下角出现状态条：`OCR…` → `OCR ✓ (N blocks)`。
3. 然后 `Shift` 悬停图里的日文——Yomitan 像在普通文字页一样弹窗。
4. 点 Yomitan 弹窗的 `+` 加 Anki，例句就是那个气泡的 OCR 文本。

每张图只 OCR 一次并缓存，之后悬停秒出。

### Debug 可视化

页面左下角有个眼睛图标（👁），点一下切换识别框可视化：

- 红框 = 检测到的 block
- 绿框 = block 内的每一行 line
- 半透明红字 = 识别出的文本

新站点验证对齐时很有用。

### 已知限制

- **只支持 `<img>` 元素**，不识别 CSS `background-image` 和 `<canvas>`。
- **跳过 200×200 以下的图**（避免 OCR 头像/图标）。要改的话改脚本里的 `MIN_DIM`。
- **每张图首次 OCR 1–2 秒**，第一次按 `Shift` 要按久点。
- **OCR 准确度看图本身**。普通对话气泡很准；手绘音效、花体字、太小的字常 miss。
- **CSP 严格的站点**（比如 x.com 部分页面）需要 `GM_addStyle` 才能注入样式。脚本已经用了；如果你的油猴管理器没批这个权限，样式不生效会出现错位。
- **OCR 服务只接受本地连接、无认证**。它监听 `127.0.0.1`，会解码你发给它的图片；不要把它暴露在公网。

### 故障排查

| 现象 | 可能原因 |
|---|---|
| Console 显示 `OCR result: 0 blocks` | mokuro 没检测到文字。换更清晰/更大的图试试。 |
| Console 显示 `network error` | OCR 服务没启动，或端口不是 `7331`。 |
| `OCR ✓` 出现但 Yomitan 不弹 | 该域名 Yomitan 被禁用，或修饰键不是 `Shift`。 |
| Overlay 渲染了（debug 能看到框）但 Yomitan 还是不弹 | 站点拦截了图片的鼠标事件。把 URL 提 issue。 |
| 悬停后图片上出现一片灰色矩形挡住文字 | 旧版的文字选中高亮 bug。升级到 v0.2.0+。 |

### 致谢

本项目只是把以下更难的工作粘合起来：

- [mokuro](https://github.com/kha-white/mokuro)——检测+识别 pipeline，以及"漫画 HTML 文字层叠加"这个原始想法
- [manga-ocr](https://github.com/kha-white/manga-ocr)——识别模型
- [comic-text-detector](https://github.com/dmMaze/comic-text-detector)——检测模型
- [Yomitan](https://yomitan.wiki/)——所有真正的查词工作都是它做的

### 协议

MIT——见 [LICENSE](LICENSE)。
