# ft8rs GUI 开发计划（草案 v2）

> 状态：**仅计划，未开始开发**。设计草图待补充（见 §12）。
> v2 并入第三方 review 的修正，并收敛范围：**GUI 只做 monitor 模式**。

---

## 1. 目标与范围

- CLI 与 GUI **相互独立**，编译为**两个可执行文件**，共享同一套解码核心。
- 第一期平台：**macOS（本地构建）+ Windows（在线 CI 构建）**。Linux 暂只保留 CLI。
- **范围收敛（决策 9）：GUI 只服务 monitor（实时声卡）模式。file（离线 WAV）模式留在
  CLI，不进 GUI**——CLI 现有同步路径已足够；engine 因此**只需服务 live**，不必实现可取消
  的逐 slot 文件回放（这也消除了 review 指出的「file 同步读全量会卡 actor」问题）。
- GUI 相对 CLI 的核心增量：**几乎所有 monitor 参数可在运行中动态修改**——声卡、
  mycall/hiscall、profile、频率窗口、SWL/nagain、UDP 等。每个参数都要有明确的
  「能否切换 / 切换前做什么 / 切换后做什么 / 哪些状态保留、哪些复位」方案（见 §6）。
- 目录结构调整为 **CORE / ENGINE / CLI / GUI**（Cargo workspace，见 §3）。
- **硬约束 1（不可碰解码）**：**不得改动 `lib_wsjtx` / `lib_jtdx`**，必须保持与原版
  WSJT-X / JTDX 的对齐。所有新增都在解码核心**之上**的编排层，绝不进入解码内核。
- **硬约束 2（回归基线不破）**：验收按 §10 的**三类**口径，而**不是笼统的「465 行
  byte-identical」**——465 是 hybrid 长录音**总解码数的精确计数回归**，其它主测试是
  **灵敏度 floor**（如 wsjtx 长 ≥424），逐位对齐是另一类源码审计测试。
- **硬约束 3（灵敏度不降）**：必须持续通过解码灵敏度测试，任何改动不得降低灵敏度，
  每个里程碑复跑灵敏度基线。

非目标（第一期不做）：file 模式纳入 GUI、QSO 自动应答/发射（TX）、日志数据库、声音回放
编辑。频谱/瀑布见 §9，**二期**做。

---

## 2. 现状分析（含 review 核实的“尚不存在的能力”）

读码结论 + review 核实，逐条对应到后续设计：

1. **核心已 UI 无关**：`decode/`、`stream/`、`input/`、`util/`，GUI 可复用。
2. **配置在构造时固化**：`StreamDecodeConfig`（`src/stream/session.rs:143`）在
   `ProfileStreamDecodeSession::new(config)` 时被复制进各 profile 内部状态。→ 改任何解码
   参数 = 重建对应 session。
3. **统一接口无状态迁移**：`ProfileStreamDecodeSession`（`src/stream/profile.rs:17-57`）
   只有 `new` + `decode`，**没有 export/import/reconfigure**。现存的跨 session 迁移只有
   `import_hash_calls`（`session.rs:394`、`lib_jtdx/mod.rs:108`）+ jtdx 的
   `export_regular_hash_calls`。→ **「重建 session 仍保留跨时隙记忆」需要先建迁移契约**，
   不是现成能力。
4. **session 归 worker 局部所有**：`src/input/soundcard.rs:234`、`:323` 里 session 是
   decode worker 线程的局部变量。→ 换设备若拆 worker 必丢 session，**必须 Capture/Decode
   actor 分离**（§4）。
5. **采集端永远重采样到 12k**：解码核心不感知声卡采样率/格式。→ 换声卡天然与解码状态无关。
6. **DX 情报全绑定在 target 上**：`TargetContextStore`（`src/decode/dx/context.rs`）的
   frequencies/parity/hisgrid/dt。但 **`frequencies` 是扁平 `Vec`，无来源字段**
   （`context.rs:36`），现仅靠 `confidence` 重载间接区分。→ 想在换 mycall 时只清 S5/留 S4，
   **必须先加 `FrequencyOrigin`**（§6.5），否则只能整体复位 DX 情报。
7. **统一接口只吐裸 `StreamDecodedMessage`**（`profile.rs:40`），provenance 在 hybrid/jtdx
   内部有但被丢；DX context **无公开 snapshot**。→ GUI 需要的 provenance / DX 情报面板
   **无底层支撑，需新协议**（§4）。
8. **core 内有瞬时并发线程**：hybrid/dx 用 `thread::scope`（`hybrid/mod.rs:85`、`dx/mod.rs`）。
   → 表述应为 core **「不拥有常驻运行时线程/设备生命周期」**，而非「无线程」。
9. **输出层在 bin 里**：`src/output/{cli,udp}.rs`；UDP 可共享下沉 engine，stdout 留 CLI。
10. **构建脆弱点**：`build.rs` 的 `cargo:rustc-env` 只对**本 package**生效，
    `copy_allcall7_to_binary_dir` 依赖 `CARGO_MANIFEST_DIR`（`build.rs:42`）。→ 简单地把
    `build.rs` 放进 core **无法**给 CLI/GUI 注入版本、也找不到根目录的 `ALLCALL7.TXT`
    （静默跳过），必须重做（§8）。

### 2.5 开工前必须先定的契约（review 的核心结论）

不要在动 workspace 前假设下列能力已存在；它们决定后续是顺滑推进还是边做 GUI 边拆发动机：

1. **构建/版本/资源契约**（§8）。
2. **per-profile session 状态迁移契约**（§5.3）。
3. **Capture / Decode actor 所有权模型**（§4）。
4. **验收三类口径**（§10）。

---

## 3. 目录结构（4-crate workspace：core / engine 分开）

切分原则：**core = 不依赖 cpal、不拥有常驻线程/设备生命周期的“纯零件”**（解码内部的
`thread::scope` 瞬时并发是允许的）；**engine = 拥有实时音频与编排的“运行时”，仅服务 live**。

```
ft8rs/                          # workspace 根
├── Cargo.toml                  # [workspace] members + 公共 profile(release/fast)
├── Cargo.lock
├── crates/
│   ├── ft8rs-build/            # ★ 共享 build helper（git/tag → 版本串），build-dependency
│   ├── ft8rs-core/             # 库：纯解码核心，无 cpal / 无常驻线程 / 无设备
│   │   ├── Cargo.toml          # deps: rustfft, hound, num-bigint
│   │   ├── build.rs            # ← FFTW 链接 + ALLCALL7 拷到 bin 目录（见 §8；不注版本）
│   │   ├── ALLCALL7.TXT        # ★ 资源放在 core crate 内（见 §8 说明，勿放 workspace 根）
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── decode/         # ← 原样迁移（lib_wsjtx/lib_jtdx 不可改，硬约束 1）
│   │       ├── stream/         # ← 原样迁移（session/slot/time/profile）
│   │       ├── input/
│   │       │   ├── audio.rs    # ← WAV 读取 + resample_linear（纯函数）
│   │       │   └── file.rs     # ← WAV 文件解码编排（CLI file 模式用）
│   │       └── util/           # ← FFT
│   ├── ft8rs-engine/           # ★ 库：运行时/编排器，拥有实时音频；仅 live
│   │   ├── Cargo.toml          # deps: ft8rs-core, cpal
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── capture.rs      # ★ Capture actor：cpal stream + 采样 + 重采样 + 时隙对齐
│   │       ├── decode_actor.rs # ★ Decode actor：长期持有 session，消费帧产出 event
│   │       ├── protocol.rs     # ★ 命令/事件/快照协议（§4）
│   │       ├── reconfig.rs     # ★ 重配级别 + 状态桶迁移/复位规则（§5/§6，可单测）
│   │       └── report.rs       # ★ 共享输出：UDP 报告（原 output/udp.rs 下沉，失败隔离）
│   ├── ft8rs-cli/              # bin: ft8rs（保持现有 CLI 行为/输出不变；file + monitor 都在）
│   │   ├── Cargo.toml          # deps: ft8rs-engine（含 core）, clap
│   │   ├── build.rs            # 调 ft8rs-build 生成本 bin 的 FT8RS_VERSION
│   │   └── src/
│   │       ├── main.rs         # ← 原 src/main.rs（file 走 core 离线路径，monitor 走 engine）
│   │       └── output_cli.rs   # ← 原 output/cli.rs（stdout 格式化，CLI 专属）
│   └── ft8rs-gui/              # bin: ft8rs-gui（egui/eframe；monitor only）
│       ├── Cargo.toml          # deps: ft8rs-engine（含 core）, eframe/egui
│       ├── build.rs            # 调 ft8rs-build 生成本 bin 的 FT8RS_VERSION
│       └── src/
│           ├── main.rs
│           ├── app.rs          # eframe::App，持 EngineHandle，渲染 + 发命令
│           ├── view/           # 控件面板、解码表、DX 情报面板、状态栏
│           └── state.rs        # GUI 侧设置/视图状态（持久化：设备用 host+name，非 index）
└── *.md                        # 文档保留在根；core 基线测试迁到 crates/ft8rs-core/tests/
```

依赖方向（单向，无环）：`gui|cli → engine → core`；`cli|gui` 的 build.rs → `ft8rs-build`。

要点：
- **core 不依赖 cpal**；CLI 的 **file 离线解码完全在 core**（不进 engine）。
- **engine 独占 cpal + Capture/Decode actor + 重配策略 + 输出 sink，且只服务 live**。
- **CLI monitor 保留现有阻塞管线（不接 actor）**；只有 **GUI 用 actor engine**。这样 P0
  CLI「零行为变更」与 actor 引入的失败隔离等新行为互不冲突（见 §9、§4）。
- **版本注入在各 bin 的 build.rs**（core 不注版本）；**ALLCALL7 定位逻辑在 lib_jtdx 内、
  不可改**，故资源放 `crates/ft8rs-core/` 以匹配其编译期兜底路径。详见 §8。

---

## 4. 运行时架构：Capture actor + Decode actor（monitor only）

review 指出：当前 session 归 worker 局部所有，换设备拆 worker 必丢 session。修正为**两 actor
分离**，让“换设备保留情报”自然成立，也免去跨线程搬运大 session：

```
                 commands (mpsc)
GUI ───────────────────────────────▶ Decode actor ──(spectrum/event)──▶ GUI
(eframe::App)                          │ 长期持有 session (S1..S5) + sinks(S6)
      ▲  events (有界/coalesce)        │ 收 (ts, samples_12k) 帧
      └────────────────────────────────┤
                                        ▼ 控制 / 重启
                                  Capture actor
                                  │ cpal Stream (S0) + 采样 + 重采样12k + UTC时隙切片
                                  ▼ 产出 (ts, samples_12k) 帧
```

- **换设备 = 只重启 Capture actor**，Decode actor 不动 → **S1~S6 全保留**（含 DX 情报）。
- 命令（GUI → engine，**monitor 专属，无 OpenFile**）：
  `StartMonitor{device,config}` / `StopMonitor` / `ApplyState{device,config}`（提交期望完整
  状态，引擎 diff 决定动作）/ `RefreshDevices` / `Shutdown`。
- **正式协议（Phase 1 定义，是 engine↔GUI 的契约，不是 GUI 小细节）**：
  - `SessionEvent`：`Decode{ts,row,provenance}` | `SlotComplete` | `Status` |
    `DevicesRefreshed` | `DxContextSnapshot` | `ReconfigOutcome{level,reset,migrated}` |
    `Error` |（二期）`SpectrumFrame`。
  - `SessionSnapshot` / `DxContextSnapshot`：状态导出，供 session 迁移与 UI 展示。
  - `ReconfigOutcome`：引擎 diff 后**实际执行的动作**（级别 + 复位桶 + 迁移项），UI 据此
    清/留对应面板。
- **通道策略（防反压）**：`Decode` 行有界排队；`Status`/`SpectrumFrame`/`DxContextSnapshot`
  用**覆盖/coalesce**（只留最新值）。GUI 卡顿不得反向阻塞解码。
- **错误隔离**：音频错误 → 设备消失检测 → `Error` 事件 + 重连策略（不 `panic`、不止于
  `eprintln`）；UDP DNS/bind/send 失败 → 保留旧 sink 或自动禁用 + 状态事件，**绝不终止解码**。
- **`ApplyState` diff 语义**：一次提交可能多字段同时变（如同时改 mycall + nfa）。引擎对每个
  变更字段算重配级别，**取最大级别执行一次**，复位桶取**并集**，迁移桶取交集——一次切换，
  不逐字段反复重建。
- **保留 wsjtx 早解码分级管线（决策 12：GUI 也要早解码）**：现 soundcard 对 wsjtx 走 nzhsym
  41/47/50 三段早解码降低延迟（`soundcard.rs:60-147`，非 wsjtx 只在 50 解码）。**GUI 同样保留
  早解码**，故 Capture actor 必须**按段产出子时隙帧**，否则 wsjtx 实时延迟变差、且 P2「与现状
  一致」DoD 不成立。→ actor 协议支持「同一时隙的 41/47/50 三帧」，重配只在 50（时隙末）与下一
  时隙 41 之间落地。早解码的部分结果经 `Decode` 事件即时上屏，`SlotComplete` 在 50 后发。
- **DX live watchdog（自查新增）**：现 monitor 对 DX 设 `dx_monitor_watchdog_ms = 12_000`
  （`main.rs:19,238`，给 focused 解码的每时隙时间预算，防超时）。engine 跑 DX live 时必须
  同样设置此 config 字段（纯配置，不改代码）。

---

## 5. 重配级别 + 状态桶（修订）

### 5.1 重配级别
- **L0 仅输出**：重建/丢弃 UDP sink、改 sink 状态。即时生效。
- **L1 重建 session（保采集+时隙）**：drop 旧 session、用新 config 建新 session，**在时隙
  边界切换**。重建后按迁移契约（§5.3）注回可迁移的跨时隙记忆，其余复位。
- **L2 仅重启 Capture actor（重对齐）**：拆/开 cpal，读新采样率，重对齐下一个 UTC 时隙。
  **Decode actor 不动，S1~S6 全保留**。代价：丢约 1 个时隙（UI 提示）。

### 5.2 状态桶（修订 S1 定义）
| 桶 | 内容 | 归属 / 现状 |
|----|------|------|
| **S0 采集** | cpal stream、collector carry、采样率、时隙时钟 | Capture actor |
| **S1 内核 scratch** | 解码内核**每时隙重置**的工作缓冲（`Ft8Mod1` 工作区、ft8b workspace；`reset_for_slot`） | **重建即重置，无需也无法跨重建保留；重建廉价** |
| **S2 hash 簿** | 学到的 hash 呼号 | **可迁移**（`import_hash_calls` 已存在） |
| **S3 跨时隙 AP 记忆** | wsjtx A7、jtdx 奇偶 AP、hybrid evidence | **目前无导出接口**，需新契约 |
| **S4 DX 目标情报** | foci/frequencies、observed parity、收割 hisgrid、dt、Fox 网格、`DxTarget`——hiscall 派生 | 需 `FrequencyOrigin` 才能精确拆（§6.5） |
| **S5 DX 操作员情报** | inferred parity、recipient/mycall 频率置信——mycall 派生 | 同上 |
| **S6 输出** | UDP sink、解码日志 | Decode actor / 输出层 |

### 5.3 session 状态迁移契约（新增，Phase 1 产出）
- 重建 session **不保 S1**（scratch，无意义）；要保的是 S2 / S3 / S4 / S5。
- **现状只有 S2 可迁移**。S3（A7/奇偶 AP/evidence）、S4/S5（DX 情报）需要新增
  `SessionSnapshot` / `DxContextSnapshot` 的 export/import。
- 原则：**能证明安全迁移的才迁移，证明不了默认复位**。Phase 1 必交付「逐 profile 可迁移项
  清单 + 测试」，未证明项一律保守复位（结果仍正确，只是少留一点记忆）。
- **S3 迁移可延后（自查补充）**：不迁移 S3 只会在**切换后 1 个时隙内**损失跨时隙 AP 记忆，
  属一次性瞬时影响、且只在用户主动切换时发生；离线灵敏度测试（file 模式、无切换）不会触发，
  故**不算违反硬约束 3**。先做 S2，S3 迁移作为后续可选优化（若实测对 live 灵敏度敏感再补）。

---

## 6. 参数动态切换详表（修订：不再笼统“保留 S1”）

「迁移桶」= 重建后注回的跨时隙记忆（受 §5.3 契约约束，括号项为契约就绪后才迁移）。

| 参数 (CLI) | 在线切换 | 级别 | 复位 | 迁移/保留 | 切换前 | 切换后 / 备注 |
|---|---|---|---|---|---|---|
| UDP 开关 `--udp` | ✅即时 | L0 | sink | 全部解码态 | — | 建/丢 `UdpOutput`；失败隔离不停解码 |
| UDP host/port `-o/-p` | ✅即时 | L0 | sink | 同上 | 校验地址 | rebind；失败保留旧 sink + 状态事件 |
| 内核开关 `filter`/`hide_dupes`/`hide_hash` | ⏳边界 | L1 | S1 | 迁移 S2(+S3) | — | **解码内核开关**（带宽/候选/线程/内核去重），非显示过滤（§6.3）。DX 下置灰 |
| 频率下限 `--low`→nfa | ⏳边界 | L1 | S1 | 迁移 S2(+S3)；DX：S4 按新带裁剪 | 校验 low<high、clamp | 重建 session；DX 剔除越界 foci（需 origin） |
| 频率上限 `--high`→nfb | ⏳边界 | L1 | 同上 | 同上 | 同上 | 同上 |
| 收听频率 `--rx-frequency`→nfqso | ⏳边界 | L1 | S1 | 迁移 S2(+S3)、S4/S5 | clamp 到 [nfa,nfb] | 重建；DX 追加 `UserPinned` 焦点 |
| 我的网格 `--my-grid`→mygrid | ⏳边界 | L1 | S1 | 迁移全部跨时隙记忆 | 校验 grid | 重建；影响很小 |
| 对方网格 `--his-grid`→hisgrid | ⏳边界 | L1 | S1 | 迁移 S2(+S3)、S4 | 校验 grid | 重建；DX 覆盖 `context.hisgrid`（选项 C，§6.4） |
| 我的呼号 `--my-call`→mycall | ⏳边界 | L1 | S1；DX：**S5** | 迁移 S2(+S3)、S4 | 规范化+校验 | 重建+re-seed hash；DX 仅复位 mycall 派生（**需 §6.5 origin**，否则整体复位 DX 情报） |
| 对方呼号 `--his-call`→hiscall | ⚠️边界+确认 | L1 | DX：S1+S4+S5 全清 | DX：S0+S2 | DX **二次确认**（丢弃目标情报，可归档旧日志） | DX 整建 `DxStreamDecodeSession`；UI 清 DX 面板（§6.1）。非 DX：仅 re-seed hash |
| profile `--profile` | ⏳边界 | L1 | S1,S3，进出 DX 连带 S4/S5 | S2 可重导 | dx 需 hiscall | 换 session 类型=新建；UI 提示情报重置 |
| SWL `--swl` | ⏳边界 | L1 | S1 | 迁移 S2(+S3) | — | 重建；DX 内部强制 on → DX 下置灰 |
| nagain `--nagain` | ⏳边界 | L1 | S1 | 迁移 S2(+S3) | — | 重建；DX 内部按焦点强制 → DX 下置灰 |
| 声卡设备 `--device` | 🔁仅重启 Capture | L2 | S0 | **S1~S6 全保留** | 列举校验、提示丢约 1 时隙 | **只重启 Capture actor**，Decode actor 与 DX 情报不动（§6.2） |
| 监听 启/停 | 🔁 | L2 | S0 | 其余保留 | — | 起停 Capture actor；Decode actor 与记忆保留 |

图例：✅即时 / ⏳时隙边界 / ⚠️需确认 / 🔁重启采集。

### 6.1 DX 换 `hiscall`
- **非 DX（wsjtx/jtdx/hybrid）**：hiscall 仅 hash seed + AP 提示，切换=重建 + re-seed hash，廉价。
- **DX**：hiscall 定义整个 target，`DxTarget`/`hash_seed_calls`/`TargetContextStore` 全失效。
  1. 切换前：校验 + **弹确认**（将丢弃全部已收集情报）；可选归档旧目标日志。
  2. 切换后：整建 `DxStreamDecodeSession`（仅 nfqso 作初始 pinned seed、新 hash、新 listen）。
  3. 复位 S4+S5；UI 清 DX 情报面板。保留 S0（不重对齐，属 L1）+ S2。
  4. 旧目标自动收割的 hisgrid 必须清；新目标用用户填的 his-grid，否则等待重新收割。

### 6.2 换声卡（修订为 Capture-only 重启）
采集端永远重采样到 12k、解码不感知设备 → 换声卡是 **L2 仅重启 Capture actor**：拆旧 cpal →
开新设备（读采样率/格式）→ 重对齐下一个 UTC 时隙。**Decode actor 与 S1~S6 全保留**（操作员
还在追同一个 DX）。代价：重对齐丢约 1 个时隙（UI 提示）。样本格式/声道由 capture 处理。

### 6.3 `filter`/`hide_dupes`/`hide_hash` 是解码内核开关，不是显示过滤
- `filter`：收窄解码带宽到 `nfqso±60/±290`、候选阈值 ×3、线程封顶 8（`lib_jtdx/mod.rs:390`、
  `sync8.rs:106`）。`hide_dupes`：改内核去重判据（`mod.rs:1071`）。`hide_hash`：含 `<...>`
  的行**直接不产生**（`decode_helpers.rs:524`）。
- 三者 baked 进 session → **L1**。DX 内部强制 `filter=false`（`dx/mod.rs:204/218`），GUI 下置灰。
- **已确认（决策 3）：不做任何显示过滤/视图层筛选**。GUI 直接展示引擎产出行。

### 6.4 hisgrid：选项 C（软化风险表述）
机制：DX 每收到 target-sender 行，`harvest_grid` 无条件覆盖 `hisgrid`（`context.rs:268`），
且 harvest 跑在所有 listen 结果上、**不受 emit 抑制影响**（`dx/mod.rs:80-86`）。
`hisgrid` 被 a8d focus 与 `has_hard_grid_contradiction`（抑制矛盾的 target-sender 行）使用。
- **已确认（决策 4）= 选项 C**：自动收割可覆盖 + GUI 始终显示生效 hisgrid 及来源（用户填/
  收割），操作员可随时重填。
- 风险（修正措辞）：错误 grid 在选项 C 下**最多隐藏“首个矛盾的 sender 行”**，下一时隙 listen
  收割即自纠；只有**硬锁（选项 B）**才会持续隐藏——这正是不选 B 的理由。

### 6.5 FrequencyOrigin（S4/S5 精确拆分的前置）
当前 `FrequencyCandidate` 无来源，仅 `confidence` 重载，无法可靠删“仅旧 mycall 派生”的频率。
落地：

```rust
enum FrequencyOrigin { TargetSender, TargetRecipient, MyCall, UserPinned }
```

给 `FrequencyCandidate` 加 `origin`，才能实现 S4/S5 的精确裁剪/迁移（换 mycall 只清 `MyCall`/
`TargetRecipient` 派生项、保留 `TargetSender`/`UserPinned`）。**未落地前，换 mycall 在 DX 下
保守整体复位 DX 情报**。这是 DX 来源化情报阶段（P4）的前置，且全在 dx 编排层（不碰 lib_*）。

---

## 7. GUI 技术选型：egui（频谱由 engine 独立计算）

**已确认（决策 1）= egui + eframe**：纯 Rust 单二进制、mac/win 一等支持、即时模式契合仪表盘、
与 channel 化引擎天然契合、内置设置持久化。

- **频谱/瀑布（二期）**：**engine 从 12k 音频独立算一条展示用 FFT**（不导出、不依赖内核内部
  频谱 → 契合硬约束 1，也避免“导出内核频谱未必可行”的坑），经 `SpectrumFrame` 事件
  （coalesce）给 GUI；egui 用动态纹理（`ColorImage`/`TextureHandle`）渲染瀑布、`egui_plot`
  或自绘画频谱曲线。

> 等设计草图到位再定具体布局；框架先按 egui 推进 §9 P3 脚手架。

---

## 8. 构建 / 版本 / 资源（重写）

review 指出且自查确认两处必须分别处理（且**不能碰 lib_jtdx**）：

**版本注入**（`cargo:rustc-env` 只对本 package 生效）：
- 抽出 `ft8rs-build` 共享 helper（封装 git/tag → 版本串逻辑），**作为 build-dependency 库
  crate**，由 **cli 与 gui 各自的 `build.rs` 调用**生成各自的 `FT8RS_VERSION`（不复制脚本、
  去重；决策 11）。core 不注版本。
- helper 须处理 **`.git` 在 workspace 根、而非 package 目录**：git 命令的 cwd 与
  `rerun-if-changed=.git/...` 路径要用 workspace 根（向上回溯或绝对路径），否则版本
  探测与增量失效。

**资源 `ALLCALL7.TXT`**（关键自查修正）：
- 运行时定位逻辑在 **`lib_jtdx/searchcalls.rs:50-62`，属硬约束 1，不可改**。它按序找：
  ① exe 同目录、② cwd、③ **编译期 `env!("CARGO_MANIFEST_DIR")`/ALLCALL7.TXT**。
- 因为 ③ 在编译期展开成 **该文件所属 crate（= `ft8rs-core`）的目录**，所以
  **`ALLCALL7.TXT` 必须放在 `crates/ft8rs-core/`**（不能放 workspace 根，否则 dev
  `cargo run` 兜底失效）。→ 故**放弃 v1 的 `assets.rs`/`include_bytes!` 方案**（那会要求
  改 searchcalls.rs = 违反硬约束 1）。
- core 的 `build.rs` 保留 `copy_allcall7_to_binary_dir`（源 = core 的 `CARGO_MANIFEST_DIR`，
  目标 = `target/<profile>/`），使 `cargo run` 的两个 bin 经 exe 同目录命中（① 路径）。
- **打包放置**：Windows zip 放 exe 旁；macOS `.app` 放 `Contents/Resources` **且需在 exe
  同目录或 cwd 可达**——因为 searchcalls 只查 exe-dir/cwd/编译期路径，**不查
  `Contents/Resources`**。故 macOS 要么把 `ALLCALL7.TXT` 放进 `Contents/MacOS/`（与可执行
  同目录），要么启动时把 cwd 设到资源目录。这条是 `.app` 打包脚本的明确约束。

**CI / profile**：
- 双二进制 `cargo build --release -p ft8rs-cli -p ft8rs-gui`；Windows `windows-latest` 同构
  cli+gui 打 zip（`ALLCALL7.TXT` 放 exe 旁）；macOS 本地构建 `.app`（§8.1）。
- `release`（thin LTO + 单 codegen-unit）与 `fast` 提升到 workspace 级，语义不变。
- **feature 透传**：`fftw` feature 现在 `ft8rs` 上，拆分后随 FFT 代码归 `ft8rs-core`；需要
  FFTW 的 bin/test 要透传 `ft8rs-core/fftw`（CI 的 FFTW 用例命令相应改写）。这是 workspace
  特性统一的常见坑，P0 一并处理。

### 8.1 macOS 打包（决策 6）
**已确认 = `.app` bundle（未签名亦可）**：含 `Info.plist`（bundle id、图标、且**必须有
`NSMicrophoneUsageDescription`**——现代 macOS 把所有音频输入含虚拟声卡纳入“麦克风”隐私门，
裸二进制无此串会授权异常）。**`ALLCALL7.TXT` 放 `Contents/MacOS/`（与可执行同目录）**，而非
惯例的 `Contents/Resources`——因为 searchcalls 只查 exe-dir/cwd（§8），不查 Resources。
codesign + notarize 留四期/公开分发时。Windows 侧类似但更轻、无硬性要求。

---

## 9. 里程碑与阶段（按 review 重排；monitor-only 简化）

> 每阶段 DoD 都隐含**硬约束 1/2/3**：不碰 `lib_*`、§10 三类回归不破、灵敏度不降。

**P0 — Workspace / 构建 / 资源 / CI（零行为变更）**
- 拆 `ft8rs-build` + `ft8rs-core`（纯解码，无 cpal）+ `ft8rs-engine`（cpal + 现有 soundcard
  阻塞管线 + UDP 下沉）+ `ft8rs-cli`。落地 §8 的版本/资源/feature 透传方案；基线测试迁到
  `crates/ft8rs-core/tests/`；更新 CI 构建 cli（含 FFTW 用例改 `ft8rs-core/fftw`）。
- **DoD**：既有测试不改一行通过；`ft8rs` CLI（file + monitor）行为/输出/`--version` 与现状
  一致；灵敏度基线不降。

**P1 — session 能力 / 快照契约（先证明可迁移项）**
- 定义 `SessionSnapshot` / `DxContextSnapshot` / `ReconfigOutcome`；逐 profile **证明**哪些
  跨时隙状态可安全迁移（先证明、再写迁移），未证明项默认复位。
- `reconfig.rs` 单测：reconfig planner（旧/新 config+device → 级别 + 复位/迁移桶），纯逻辑、
  无音频、确定性，覆盖 §6 全部行。
- **DoD**：可迁移项有测试佐证；planner 测试全绿；灵敏度不降。

**P2 — Capture + Decode actor + 可取消命令协议（仅 live engine）**
- 在 `ft8rs-engine` 实现两 actor 分离 + command/event 协议（§4），全在解码核心之上。
  **不实现 file 回放**（file 留 CLI 同步路径）。**CLI monitor 仍走原阻塞管线、不接 actor**
  （actor 只服务 GUI），避免改动 CLI 行为。
- Capture actor **保留 wsjtx nzhsym 41/47/50 早解码分级**（§4）；DX live 设 watchdog（§4）。
- **DoD**：engine 驱动 live 解码与现状一致（含 wsjtx 早解码时序）；`Stop`/`ApplyState` 及时
  响应（不被解码阻塞）；CLI 行为不变。

**P3 — 最小 GUI（monitor）**
- 设备选择、启/停、profile + 呼号/网格/频率窗口控件、实时解码表、状态栏；接 engine；
  落地可靠的 L0/L1/L2 在线切换。
- **DoD**：能监听、能在线改 profile/呼号、能看解码；mac 本地可运行 + Windows CI 出
  `ft8rs-gui.exe` 工件。

**P4 — DX 体验 + 声卡热插拔**
- 落地 `FrequencyOrigin` 来源化情报（§6.5）→ `DxContextSnapshot` 情报面板（foci/parity/grid+
  来源标记/dt）；hiscall 切换确认 + 情报复位（§6.1）；声卡热插拔（Capture-only 重启，§6.2）。
- **DoD**：§6 整张矩阵实现并人工验证；灵敏度不降。

**P5 — 频谱 + 持久化 + 打包**
- engine 独立 FFT 频谱（§7）；设置持久化（设备用 host+name 等稳定标识，**非 index**）；
  macOS `.app`（§8.1）+ Windows 安装器；notarize 视分发需要。

---

## 10. 测试与回归保证（三类验收，修正 465）

- **类 A — 精确计数快照**：hybrid 长录音 `assert_eq!(total, 465)`
  （`HYBRID_LONG_TARGET_COUNT`，`tests/stream_decode_test.rs:11`、`:605`）。
- **类 B — 灵敏度 floor**：`matched >= floor`，wsjtx 长 ≥424、jtdx 长 ≥430、短 ≥19/20
  （`LONG_TARGET_ACCEPTED_FLOOR` 等，对 baseline CSV）。
- **类 C — 可复现 + 耗时**：`assert_release_mode()` + 每时隙时间预算。
- **lib_* 红线（硬约束 1）**：code review 把“是否改了 `lib_wsjtx`/`lib_jtdx`”列为红线。
- **逐位对齐**属 `wsjtx_source_audit_test.rs` 这类源码审计，与上面三类不同，**勿混为
  “465 byte-identical”**。
- engine 逻辑（planner、状态迁移、actor 协议）用纯单测覆盖；CLI 文件回放可做手动验证清单
  （每个参数走一遍 §6）。
- 日常 `cargo test --profile fast`，最终 `--release`（含 FFTW）验收。

---

## 11. 决策记录

### 11.1 已确认
- **硬约束**：① 不碰 `lib_wsjtx`/`lib_jtdx`、保持对齐；② §10 三类回归不破；③ 灵敏度不降。
- **决策 1 — GUI 框架 = egui/eframe**（频谱二期由 engine 独立 FFT 计算，§7）。
- **决策 2 — 4(+1) crate**：`ft8rs-build`/`core`/`engine`/`cli`/`gui`；core 纯解码无 cpal，
  engine 为运行时（cpal + actor + 重配 + UDP）。依赖 `gui|cli → engine → core`（§3）。
- **决策 3 — 不做显示过滤**（§6.3）。
- **决策 4 — hisgrid 选项 C**（收割可覆盖 + UI 显示来源，§6.4）。
- **决策 5 — 换声卡保留 DX 情报**：L2 仅重启 Capture actor，S1~S6 保留（§6.2）。
- **决策 6 — macOS `.app`**（Info.plist + 麦克风权限串；`ALLCALL7.TXT` 放 `Contents/MacOS/`
  而非 Resources，因 searchcalls 只查 exe-dir/cwd，§8.1）。
- **决策 7 — Linux GUI 第一期不做**，仅 CLI。
- **决策 8 — 频谱/瀑布 = 二期**，engine 独立 FFT（不导出内核频谱）。
- **决策 9 — GUI 只做 monitor；file 模式留 CLI、不进 GUI；engine 仅 live**。
- **决策 10 — 先定三契约（构建/迁移/actor）+ 三类验收口径，再动 workspace**（§2.5）。
- **决策 11 — 版本注入用 `ft8rs-build` build-dependency 库**，各 bin build.rs 调用，去重（§8）。
- **决策 12 — GUI 也保留 wsjtx 早解码分级**（nzhsym 41/47/50）：Capture actor 按段产出子时隙
  帧，早解码部分结果即时上屏（§4）。

### 11.2 待落地的前置工作项（非待决，是必做）
- `FrequencyOrigin`（§6.5）；`SessionSnapshot`/`DxContextSnapshot`/`ReconfigOutcome`（§4/§5.3）；
  `ft8rs-build` 共享 helper（§8）。这些都在解码核心之上，不碰 lib_*。

### 11.4 第二轮自查（v2.1）新增修正
- **ALLCALL7 定位在 lib_jtdx 内不可改**：放弃 `assets.rs`/`include_bytes!`；`ALLCALL7.TXT`
  放 `crates/ft8rs-core/`（匹配编译期兜底），打包仍放 exe 旁；macOS 放 `Contents/MacOS/`（§8）。
- **build.rs 分工**：core 的 build.rs 管 FFTW + ALLCALL7 拷贝；版本由各 bin build.rs（经
  `ft8rs-build`）注入，且 helper 要处理 `.git` 在 workspace 根（§8）。
- **保留 wsjtx 早解码分级**（nzhsym 41/47/50）：Capture actor 须按段产出子时隙帧（§4、P2 DoD）。
- **DX live watchdog**：engine 跑 DX 时设 `dx_monitor_watchdog_ms=12_000`（§4）。
- **CLI monitor 不接 actor**，仍走阻塞管线，避免行为变更（§3、P2）。
- **`ApplyState` 多字段 diff**：级别取最大、复位桶取并集、一次切换（§4）。
- **S3 迁移可延后**：不迁移仅切换后 1 时隙瞬时损失 AP，离线灵敏度测试不受影响（§5.3）。

### 11.3 engine 与 core 的区别
- **core =「零件」**：纯函数 + session 对象，不拥有常驻线程、不管设备生命周期、无控制循环，
  由调用方驱动；**不依赖 cpal**。
- **engine =「运行时/编排器」**：常驻 actor，拥有 cpal 流 + 长期 session + 当前 config，跑
  时隙循环，暴露 command/event/snapshot 协议，固化动态切换策略；**仅服务 live**。

---

## 12. 界面草图（已确认 v1）

主窗口：
- 顶栏菜单：**设置** | **关于**。
- 主体：解码表，列 = `UTC | dB | DT | Freq | 信息`，**与 CLI 一致**；含 `a7` 来源标记。
  **不做国家列、不做距离列**（决策：解码只有音频，避免额外数据依赖）。
- 时隙分隔行：**只显示时间**（如 `------ YYYY-MM-DD HH:MM:SS UTC ------`），不含波段/模式。
- 底部：4 个文本框 `mycall | hiscall | hisgrid | nfqso` + 一个 **Monitor** 按钮（启/停）。
- 颜色：WSJT-X 习惯（CQ 绿、含 mycall 高亮），细节实现时再调。

设置弹窗：左侧 tab 快切，右侧设置项。分类：

| Tab | 设置项 |
|---|---|
| 音频 | 输入声卡（下拉+刷新）、（只读）采样率/格式 |
| 解码 | profile（wsjtx/jtdx/hybrid/dx）、SWL、nagain |
| 频率 | low(nfa)、high(nfb)（nfqso 在底部栏） |
| 台站 | mygrid（mycall/hiscall/hisgrid 在底部栏） |
| 输出 | UDP 开关、udp_host、udp_port |
| 高级 | filter / hide_dupes / hide_hash（DX 下置灰）、DX watchdog |
| 关于 | 版本、FFT 引擎、许可证 |

默认 profile = **wsjtx**（dx 需 hiscall，空着会报错），记住上次选择。

> 构建方式已选 **引擎优先**（决策）：按 P1→P2→P3 推进，GUI 在引擎之上渲染。
</content>
