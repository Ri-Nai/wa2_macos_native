<div align="center">

<img src="launcher/wa2-native.svg" width="96" height="96" alt="wa2-native 雪花图标">

<h1>WHITE ALBUM 2 · Native for macOS</h1>

<p><strong>让白色相簿的冬天，在 Mac 上原生运行。</strong></p>

<p>用 Rust 重写的《WHITE ALBUM 2》原生引擎 · macOS Apple Silicon 移植<br>中文对白 · 原版界面 · Retina 高清 · 一条命令启动</p>

<p>
  <code>macOS · Apple Silicon</code>&nbsp;
  <code>Rust · 零第三方 crate</code>&nbsp;
  <code>2x / 4x</code>&nbsp;
  <code>Retina · 双击启动</code>
</p>

<p>
  <a href="#快速开始">快速开始</a> ·
  <a href="#游玩与操作">游玩与操作</a> ·
  <a href="#画质与设置">画质与设置</a> ·
  <a href="#常见问题">常见问题</a> ·
  <a href="#开发与调试">开发与调试</a>
</p>

</div>

---

这是《WHITE ALBUM 2》原生引擎的 **macOS（Apple Silicon）移植**，基于 [MuRongxin/wa2-linux_native](https://github.com/MuRongxin/wa2-linux_native) 的 Linux 原生重写。引擎直接读取游戏素材，重新实现剧本解释、画面合成、音频、影片和界面；运行时无需 Wine、原版 EXE 或原版安装目录。

移植本身只动了四处平台差异：把 SDL2 / FreeType 的动态加载路径换成 macOS 的 dylib、音频驱动改用 CoreAudio、字体回退换成 macOS 上真实存在的字体、窗口改用 Retina 像素尺寸。引擎逻辑与上游保持一致。

| 原生体验 | 画面与文字 |
| :--- | :--- |
| **剧情与演出** — 剧本推进、分支选择、语音、BGM、音效与影片播放 | **Retina 画质** — 1280×720 逻辑画布铺到 2560×1440，文字与立绘按设备像素渲染 |
| **熟悉的界面** — 保留游戏自己的标题、设置、存档与底部指令条素材 | **中文优先** — 默认中文，可切换日文；随仓库提供中文字体，支持字体与对白样式设置 |
| **特别模式** — CG、音乐、语音、电子小说与可交互的剧情时间线 | **独立存档** — 存档和玩家设置保存在 `data/`，更新程序后可以继续使用 |

> 当前核心流程可玩，仍在完善转场精度与长时间运行稳定性。后续工作见 [移植收尾清单](TODO.md)。

## 快速开始

需要 **macOS 13+ / Apple Silicon** 与 [Homebrew](https://brew.sh)。

~~~bash
brew install sdl2 freetype ffmpeg
git clone --depth 1 https://github.com/Ri-Nai/wa2_macos_native.git wa2-native
cd wa2-native
./run-mac.sh
~~~

**第一次运行会自动下载素材，完成后进入标题画面。** 仓库只保存源码和启动工具；素材从上游的 `data-v1.1.0` Release 获取，逐卷校验 SHA-256 后解压。环境和数据准备好后，日常启动无需联网。

| 首次下载 | 解压后的素材 | 磁盘峰值 | 首次安装建议空闲空间 |
| :---: | :---: | :---: | :---: |
| 约 **15 GB**（29 卷中的 14 卷） | 约 **14 GB** | 约 **16 GB** | 约 **20 GB** |

安装期间逐卷「下载 → 校验 → 解压 → 删除」，所以 ZIP 不会和工作树一起堆在磁盘上。解压完成后 `data/` 约 5.1 万个文件。

### 为什么比 Linux 版小很多

上游提供 2x / 4x / 8x 三套画面母版，总量 45.6 GB。macOS 版只保留 **4x 一套**：

- Retina 上最高只画到 4x（3024×1964 的显示器对应 4x 画布），8x 母版永远用不上，省掉约 22 GB；
- 2x 只是 4x 的降级备份，省掉约 4 GB；
- 1440p 影片 remaster 与 8x 立绘所在的分卷整卷跳过，省掉约 4 GB。

跳卷依据每卷 zip 中央目录里的素材树路径确定，只跳过**整卷都不需要**的分卷；跨树的分卷仍然整卷下载。所有解压出来的文件依旧来自同一个带校验的 Release，没有一处是自己生成的素材。

### 启动时会发生什么

1. **检查依赖**：直接探测 `libSDL2-2.0.0.dylib`、`libfreetype.6.dylib` 与 `ffmpeg` 是否落在 `/opt/homebrew`、`/usr/local` 或 `/opt/local` 下。缺什么就报什么，不会自动改你的系统。
2. **准备素材**：缺少 `data/script` 时运行 `launcher/fetch_data_mac.py`，用并行 HTTP Range 从 Release 拉分卷，实时显示进度与速度，支持中断续传。
3. **校验并解压**：逐卷比对清单里的 SHA-256，解压后立即删除该卷。
4. **编译并进入游戏**：`target/release/wa2-native` 不存在时自动 `cargo build --release`，然后启动。

### 双击启动

仓库根目录带一个 8 KB 的 `WhiteAlbum2.app`，只是把 `run-mac.sh` 包了一层：

~~~bash
open WhiteAlbum2.app
~~~

它不复制任何程序或素材，也不占用额外空间（素材仍然是项目里的那一份 `data/`）。脚本内部会把 `/opt/homebrew/bin` 补进 `PATH`，因为 Finder 双击启动时 `PATH` 只有 `/usr/bin:/bin:/usr/sbin:/sbin`，否则会读不到 `ffmpeg`。

### 常用启动命令

| 命令 | 用途 |
| :--- | :--- |
| `./run-mac.sh` | 进入标题画面 |
| `./run-mac.sh 1002` | 直接进入指定场景，方便调试 |
| `./run-mac.sh --check` | 只检查依赖与素材，不启动、不下载、不编译 |
| `./run-mac.sh --data` | 只准备素材，不启动游戏 |
| `./run-mac.sh --help` | 查看启动帮助 |
| `open WhiteAlbum2.app` | 双击启动（Finder 环境） |

## 游玩与操作

剧情中的常用操作：

| 操作 | 功能 |
| :--- | :--- |
| 鼠标左键 / `Space` / `Enter` / `→` | 显示完整对白，再次操作推进 |
| 鼠标滚轮向下 | 推进对白 |
| `↑` / 鼠标滚轮向上 | 回退对白 |
| `Page Up` | 开启或停止连续快退 |
| 按住 `Control`（⌃） | 快进：按住期间连续推进，松开即停 |
| 数字键 `1`–`9` / 点击选项 | 选择剧情分支 |
| 鼠标右键 | 隐藏或显示对话框 |
| `F11` / `⌃⌘F` | 切换窗口与全屏 |
| `Esc` / `Q` | 退出当前游玩 |

保存、读档、自动播放、跳过、已读文本和系统设置可通过底部指令条访问。特别模式中的内容从开始即可进入。

**时间线**支持滚轮缩放、拖动平移、单击选中节点，再次点击或按回车进入对应场景；右键或 `Esc` 返回。

## 画质与设置

`run-mac.sh` 默认 `WA2_ART_SCALE=4`、`WA2_RENDER_SCALE=auto`：素材永远从 4x 树读，解码倍率跟着窗口的实际像素走。1280×720 点的窗口在 Retina 上是 2560×1440 的绘制表面，正好是 2x，再多解码只会被缩掉。

~~~bash
WA2_RENDER_SCALE=4 ./run-mac.sh    # 强制 4x 解码（更吃内存，缩窗时不省）
WA2_RENDER_SCALE=1 ./run-mac.sh    # 最省内存，画面明显软
WA2_LANG=jp ./run-mac.sh           # 切换到日文
WA2_DATA="$HOME/Games/wa2-data" ./run-mac.sh  # 自定义数据目录
~~~

中文字体随仓库提供，包括 Noto Sans CJK SC 和得意黑。可在系统设置的文本设置中分别选择**对话文本、已读文本和 UI 字体**，并调整对白字号、描边、颜色、字重、字距与行距。

> macOS 15 起系统不再提供 `/System/Library/Fonts/PingFang.ttc`。引擎优先使用仓库自带的 `fonts/NotoSansCJK-Medium.ttc`（SC face），再回退到 Hiragino Sans GB、Songti 等系统字体；`.LastResort` 这类「任何字符都返回字形」的兜底字体已排除，否则中文会全部渲染成方块或同一个字形。

<details>
<summary><strong>更多环境变量</strong></summary>

| 变量 | 用途 |
| :--- | :--- |
| `WA2_DATA` | 数据目录，默认项目下的 `data/`；自动下载也写入此目录 |
| `WA2_LANG` | `cn` / `jp`；存在中文剧本时默认中文 |
| `WA2_ART_SCALE` | 选择素材树；`run-mac.sh` 默认 `4` |
| `WA2_RENDER_SCALE` | 实际解码倍率：`auto` 或 `1` / `2` / `4` / `8` |
| `WA2_FONT` | 高级字体覆盖，支持 `字体路径#face` |
| `WA2_FONT_HOME` / `WA2_FONT_HISTORY` / `WA2_FONT_GAME` | 分别覆盖 UI、已读文本与对白字体 |
| `WA2_ART_CACHE_MB` | 素材内存缓存预算，单位 MB |
| `WA2_ART_DISK_SCALE` | 磁盘缓存的最大倍率，默认 `2` |
| `WA2_STATS` | `1`：每秒打印一行帧率 |
| `WA2_TIMING` | `1`：打印每次场景切换各阶段耗时 |
| `WA2_FRAME_DUMP` | 把每个呈现的帧写成 TGA，调试画面用 |
| `WA2_SOFTWARE` | 强制软件渲染 |

</details>

## 程序与资源下载

素材来自上游 Release，与 Linux 版共用同一份：

| 内容 | 版本 | 本地位置 |
| :--- | :--- | :--- |
| 引擎 | 本仓库源码，本机 `cargo build --release` | `target/release/wa2-native` |
| 游戏素材 | [`data-v1.1.0`](https://github.com/MuRongxin/wa2-linux_native/releases/tag/data-v1.1.0) | `data/` |
| 中文字体 | 随仓库提供 | `fonts/` |

下载默认依次尝试：

`gh-proxy.com` → `ghfast.top` → `ghproxy.net` → GitHub 直连

失败时切换通道并保留断点，中断后重新运行 `./run-mac.sh` 即可继续。分卷缓存保存在 `.wa2-release-parts/`，不进入 Git。

~~~bash
# 仅使用 GitHub 直连
WA2_DATA_PROXIES= ./run-mac.sh

# 自定义代理顺序，最后仍会尝试直连
WA2_DATA_PROXIES='https://ghfast.top/,https://ghproxy.net/' ./run-mac.sh
~~~

<details>
<summary><strong>手动下载与已有数据</strong></summary>

**手动安装**

从 [资源 Release](https://github.com/MuRongxin/wa2-linux_native/releases/tag/data-v1.1.0) 下载需要的 ZIP 分卷和 `SHA256SUMS`，全部解压到项目目录，完成后应得到项目下的 `./data/`，不要再套一层 `data/data/`。只解压 4x 相关的树即可（`script`、`script_cn`、`voice`、`bgm`、`se`、`font`、`bg`、`bg_4x`、`sprite_4x`、`bg_cn_4x`、`movie`、`movie_2x`）。

**已有数据**

把完整的 `data/` 放到项目根目录，或通过 `WA2_DATA` 指向它。正常启动只检查关键文件和目录，不会每次重新计算全部素材的哈希。

</details>

## 常见问题

<details>
<summary><strong>为什么没有 .app 安装包或 dmg？</strong></summary>

因为这份移植是给本机自用的，不做分发。`WhiteAlbum2.app` 只是个启动壳，真正的程序和素材都在仓库目录里，改代码后重新编译即可生效，比重新打包更省事。

</details>

<details>
<summary><strong>演出变成纯黑屏 / 背景被一块灰雾盖住？</strong></summary>

两类问题都已修（`3a22a60`、`acd1165`）：

- **卷轴 CG 纯黑**：`ACT227` 原来没有实现，而它要显示的图（`v201900`、`v200000`）有好几屏高，剧本总在这之前先切黑，于是黑屏一直留着。现在按脚本给的帧数把画面向上推过一屏高度，点击可跳过剩余行程。CODA 里春希再次见到和纱的那一段就是它。
- **背景被遮住 / 像黑屏**：`ACT146/147` 的 `mode >= 0x80` 会选一张 `f0NNN` 图，那些不是背景而是**光效遮罩**（`f0000` 是太阳、`f0046` 是一片光斑、`f0024` 是水面反光），整张 alpha 都是 255、亮度即效果。以前按 alpha 贴图，等于把背景整块换成遮罩；现在改为加算叠加，人物仍然画在它上面。

后来又发现两个同源的问题（`9b18368`、`e06ce89`）：

- **多人同框时背景发黑**：`ACT146/147` 的背景 id 有三种含义，字节码里长得一样 —— `0` 是切黑、正常 id 是换图、**负数 id（只出现 `-2`，全语料 736 次）是"把当前画面重新合成一遍"**。它总是紧跟在 `spr Enter` 前面，夹在"同一张背景"和"同一张背景 + 多一个人"之间。以前把负数也当成切黑，于是人物留着、背景被涂成纯黑 —— 看起来就是"人一多背景就黑"。现在三种含义写成了显式规则（`bg_request`），负数一律保持当前画面。
- **卷轴方向反了**：原作是从**脚往上拖到头**，本移植之前是从头拖到脚，看起来像倒放。现在按记录里自带的屏数（`720, 2`）走，并做了缓入缓出，读起来像一次运镜而不是滑动。

</details>

<details>
<summary><strong>BGM 只反复循环开头几秒？</strong></summary>

早期版本会把同一个曲子的 `_A` 变体当作默认，而 `_A` 常常只是整曲的一个短片段（`BGM_001_A` 9 秒，`BGM_001_B` 86 秒；最常用的 1、2、7、41、42 号都如此）。剧本里的 `ACT158` 只带 id / 淡入帧 / 循环 / 音量四个参数，从不指定变体，所以现在由引擎优先选完整版：同名文件 → `_B` → `_A`，音乐模式的可听列表同样是这个顺序。

</details>

<details>
<summary><strong>我需要先安装 Rust 吗？</strong></summary>

需要 Rust/Cargo 1.90+ 来编译引擎（`rustup` 或 Homebrew 均可）。SDL2、FreeType、ffmpeg 由 Homebrew 提供，`run-mac.sh` 只检查不代装。Python 3 用于素材下载器，macOS 自带的就够。

</details>

<details>
<summary><strong>双击 .app 报「缺少 sdl2 / freetype / ffmpeg」？</strong></summary>

Finder 启动时 `PATH` 只有 `/usr/bin:/bin:/usr/sbin:/sbin`。`WhiteAlbum2.app` 内部已经补了 `/opt/homebrew/bin`，如果你改过 Homebrew 前缀（Intel Mac 是 `/usr/local`），确认 `run-mac.sh` 的 `lib_present` 能在那几个前缀下找到库文件。

</details>

<details>
<summary><strong>画面糊 / 字太大太小？</strong></summary>

先用 `WA2_STATS=1 ./run-mac.sh` 看帧率，再用 `WA2_TIMING=1` 看场景切换耗时。Retina 下解码倍率由 `WA2_RENDER_SCALE=auto` 根据绘制表面自动选择；强制成 `1` 会明显变软，强制成 `4` 只增加内存占用。

</details>

<details>
<summary><strong>存档和设置在哪里？</strong></summary>

存档在 `data/save/`，配置在 `data/wa2.ini`，缓存在 `data/cache/`；设置了 `WA2_DATA` 时位于对应目录。素材安装不会覆盖存档和玩家配置。引擎使用自己的存档格式，不兼容原版 EXE 的二进制存档。

</details>

<details>
<summary><strong>支持 Intel Mac 或 Windows 吗？</strong></summary>

只在 Apple Silicon + macOS 15 上验证过。Intel Mac 理论上可用（`/usr/local` 前缀已在探测范围内），但未经测试；Windows 不支持。上游的 Linux x86_64 版本请回到 [原项目](https://github.com/MuRongxin/wa2-linux_native)。

</details>

## 开发与调试

引擎使用 Rust 标准库与手写 FFI，`Cargo.toml` 的 `[dependencies]` 保持为空。SDL2 与 FreeType 通过 `dlopen` 动态加载（不是链接期依赖），音视频与图片解码由外部工具提供，没有任何 crate、`build.rs` 或 `pkg-config`。

~~~text
wa2-native/
├── run-mac.sh          macOS 启动脚本：依赖检查、素材准备、编译
├── WhiteAlbum2.app/    双击启动壳（Info.plist + 一行 shell）
├── launcher/
│   ├── fetch_data_mac.py  按素材树筛卷的素材下载器
│   ├── prange.py          并行 HTTP Range 下载
│   └── fetch_data.py      上游原始下载器（AppImage 用）
├── src/                剧本、渲染、音视频和界面实现
├── fonts/              内置中文字体与许可证
└── data/               外部游戏资源、存档、设置和缓存
~~~

<details>
<summary><strong>源码结构</strong></summary>

`src/main.rs` 只负责把各个子系统接起来并提供入口点，各子系统按职责分文件：

| 模块 | 职责 |
| :--- | :--- |
| `cli.rs` | 命令行入口与各个子命令 |
| `assets.rs` | 素材树读取：背景、光效、滤镜、界面图块、缩放策略 |
| `session.rs` | 跨场景的状态：背景索引、预取队列、播放会话 |
| `sprite.rs` | 立绘：槽位、素材目录、立绘缓存 |
| `scene.rs` | 场景合成：背景 + 光效 + 滤镜 + 立绘 |
| `game.rs` | 运行时与剧本解释器主循环 |
| `rewind.rs` | 回溯：行历史、回溯步进、存档快照 |
| `backlog.rs` | 履历画面 |
| `choice.rs` | 选项行 |
| `cmd.rs` | 命令栏与它打开的模态框 |
| `present.rs` | 转场：淡入淡出、闪光、回溯巡航 |
| `ui.rs` | 通用界面件：提示条、确认框、菜单栏、标签绘制 |
| `sound.rs` | 界面音效 |
| `gallery.rs` | CG、小说与语音信箱 |
| `special.rs` | 特别模式：番外、音乐与访问计数 |
| `title.rs` | 标题画面 |
| `movie.rs` | 影片播放 |
| `voice.rs` | 语音播放与播放规则 |
| `font.rs` | 字体加载 |
| `tests.rs` | 单元测试 |

</details>

<details>
<summary><strong>从源码构建</strong></summary>

~~~bash
cargo build --release
./run-mac.sh
~~~

~~~bash
cargo test --release
~~~

> 上游测试里有一部分把 Linux 路径（`/home/lrxin/...`）写死，在本机会失败，与 macOS 移植无关。

</details>

<details>
<summary><strong>移植改动清单</strong></summary>

| 位置 | 改动 |
| :--- | :--- |
| `src/sdl2.rs` | `dlopen` 候选加入 `libSDL2-2.0.0.dylib` 与 Homebrew 绝对路径；macOS 设 `SDL_AUDIODRIVER=coreaudio`；启用 `SDL_WINDOW_ALLOW_HIGHDPI`，窗口尺寸取 `SDL_GetWindowSizeInPixels`；固定 `delay(16)` 改为补足 16.667 ms 帧预算 |
| `src/ft2.rs` | FreeType 候选加入 `libfreetype.6.dylib` |
| `src/render/text.rs` | 字体候选改为 macOS 实际存在的字体，跳过 `.LastResort`；优先仓库自带 Noto Sans CJK SC |
| `src/audio/mixer.rs` | 音频推流改为查 `SDL_GetQueuedAudioSize` 维持约 85 ms 队列，避免 CoreAudio 下欠载产生的卡顿音 |
| `src/sound.rs` | UI 音效预解码为 PCM 并复用槽位 |
| `src/ui.rs` | 菜单标签描边预渲染；模态框按画布倍率绘制 |
| `src/assets.rs` | BGM 选完整版 `_B` 而不是短片段 `_A`；背景 id 三态化（`0` 切黑 / 负数保持当前画面 / 正常 id 换图） |
| `src/game.rs` | 实现 `ACT227` 卷轴 CG（按记录里的屏数从脚往上拖到头） |
| `src/vm/lscr.rs` | 新增 `Event::ScrollCg`，从 `ACT227` 的内联参数或其前面那条 K30 记录取出 (id, variant, frames, pages) |
| `src/render/mod.rs` | `f0NNN` 光效板改为加算（`Surface::add_scaled`），不再是按 alpha 覆盖背景 |

</details>

---

本项目为非官方引擎重写，是 [wa2-linux_native](https://github.com/MuRongxin/wa2-linux_native) 的 macOS 移植分支。《WHITE ALBUM 2》的名称与游戏素材权利归原权利方所有。随程序提供的字体许可证见 [fonts/LICENSES](fonts/LICENSES)，其他运行组件保留各自的许可证与声明。
