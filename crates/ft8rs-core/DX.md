# DX Target Decode

`--profile dx` listens for one known target over unchanged JTDX and WSJT-X
sessions. `hiscall` is required. Three independent observations supplement the
original DX tasks: the original JTDX listener, ordinary WSJT-X and a complementary
JTDX AGC path. `mycall` is optional; a valid full call pair unlocks additional
context recovery. Target-only listening uses ordinary search without inventing
the missing call.

Related notes: [JTDX](JTDX.md), [WSJT-X](WSJTX.md), [Hybrid](HYBRID.md),
[shared architecture](COOPERATIVE_DECODER_DESIGN.md).

## Execution

At the beginning of each slot, age the target context and freeze a snapshot.
The persistent JTDX SWL listener and disposable recovery tasks use that snapshot
concurrently. Context feedback is committed only after the slot's tasks finish,
in stable task order. Only the original listener and original recovery results
update the protected target context. Supplemental observations and cancellation
never change that state. No worker reads another worker's residual or AP memory.

Each selected focus receives a narrow JTDX `swl+nagain` task. When a grid is
available it can also receive a WSJT-X/a8d task. Up to five foci are selected and
at most four recovery executor threads run at once. The original JTDX parameter
`jtdx_threads` selects logical frequency partitions; the current Rust session
processes those bands serially and does not spawn one OS thread per partition.

With a user call pair but no focus, DX can run one standard full-band WSJT-X
context task with `nagain=false`. After two misses it rests for one slot. A known
focus skipped by parity policy is not mistaken for an unknown-frequency target.
Recovery sessions are private and seeded only with the user call pair.

The original listener is reused, not duplicated. Its AGC selection is preserved;
the supplemental JTDX session uses the opposite AGC selection. Both use DX's
existing SWL policy and preserve explicit logical partition settings. Hybrid and
DX share the observation execution boundary and AGC configuration factory in
`decode/cooperative/observation.rs`. Native mutable books/workspaces remain
independent; only the immutable slot samples are borrowed by multiple workers.

After these observations, an unresolved target-sender focus with eligible nearby
interference may receive one controlled WSJT-X cancellation recovery. At most two
unresolved foci and three waveform-verified seeds are used. An addressed target
never makes the other station's frequency a resolved target-sender focus. The
existing 0.70 support check, original subtraction and decoder thresholds are
unchanged. No additional JTDX residual deep search is launched.

Additional WSJT-X context requires a normalized, legal, distinct call pair;
otherwise its private call pair/grid are cleared for ordinary search. Explicit
empty `<>` in supplemental output is rejected; legitimate `<...>` is preserved.
The original DX output is never re-filtered by this supplemental guard.

## Target context

- Parse sender/recipient roles through the shared message parser. Frequency
  belongs to the sender; an addressed target does not own the sender's frequency.
- Keep user grid and pinned frequency separate from observations. Reconfiguration
  migrates valid observations and then reapplies new user values. Replacing or
  clearing a pin removes the old pin.
- Treat RR73 as an acknowledgement. Hard user-grid conflicts are rejected before
  harvest, so rejected rows cannot alter the state that rejected them.
- Native Regular target transmissions can establish grid and strong parity.
  Assisted recovery only supplies low-confidence frequency hints; it cannot
  establish grid, strong parity or independent identity.
- Update same-confidence frequencies with current observations. Native frequency
  and parity information expire after 16 slots, assisted hints after 4. An
  observed grid expires after 64 slots without native target activity. User
  values are not aged out by these rules.
- A parity contradiction clears the old lock and restores both-slot search;
  confirmation requires another distinct-slot observation. Inferred parity does
  not suppress focus searches.

## Fox/Hound foci

Actual user and observed foci are selected before predictions. Multistream
extrapolation requires at least two native target-sender observations in the same
slot, fitting the existing 60 Hz grid within tolerance. Pins, assisted hints and
observations from different slots cannot manufacture a multistream grid.

Foci are capped and bounded by the configured band. The strategy reuses existing
native inputs; it does not change native candidate ordering or thresholds.

## Output and budget

Only target-related rows passing the user-grid rule are emitted. A shared output
ledger deduplicates callbacks and final rows, preserving unresolved identities.
Monitor mode streams recovery rows immediately; file mode submits them in stable
task order. `ctx` marks the original isolated context recovery, `sic` marks
terminal cancellation recovery, and `?` preserves unknown native provenance
instead of claiming Regular. Supplemental worker errors are recorded separately
in `last_diagnostics()`; they do not erase successful protected results.

The monitor watchdog controls whether later tasks start. An original decoder
already running cannot be forcibly cancelled by this gate. Message arrival and
all-worker completion are separate measurements; evaluate the listening display
budget at the next slot +2/+3 seconds and check that tails leave room for the next
slot's early stage.

Additional cancellation has its own three-second real-time start budget. A
running native call is drained and its results are still emitted; the start
budget does not cancel decoders or affect the original DX watchdog. Observation
workers run alongside original work only when enough CPUs remain for every
original worker. On smaller machines they are deferred, and late optional starts
are skipped in real time. File mode keeps canonical unbudgeted search unless an
explicit watchdog was requested. Diagnostics separately record original task
completion/skips, all three observers, resolved-focus skips, recovery starts,
waveform checks, optional budget skips and errors.

## Validation

Run local optimized tests, including the existing a8d, long-target and synthetic
reproducibility gates. `./scripts/acceptance-gates.sh` places the 2022 calibration
CallDB beside a private test executable and leaves the live database unchanged.
Pure JTDX long-fixture calibration uses four partitions with the unchanged
430/431 threshold; see [JTDX.md](JTDX.md).

Run the additional known-waveform recovery and incomplete-context gates locally:

```sh
cargo test --profile fast -p ft8rs-core --lib decode::dx::waveform_tests:: -- --ignored --test-threads=1
```

They generate small deterministic waveforms with the unchanged native encoder;
no additional binary recordings or CI service are required.

A focus that succeeds after external seeding does not establish cold-start
sensitivity. Recovery rows must not promote their own hypotheses into strong
observations. Unannotated outputs require separate verification, and a finite
fixture set does not establish an absolute false-positive rate.

## 三路观测与目标消除：本机实验及最终开发方案（2026-10-28）

本节保留开发前对提交 `9721d5102e0fffce44829d2c764a4e8bde726a6f` 的实验和设计依据。
当时原型、合成素材和逐条日志均在独立实验目录，未改生产解码代码。
当前开发后的实现与回放结论见文末“实现后本机验证”，不得把原型数字混作开发版计时。

硬约束是原 DX 已有的目标相关消息逐条保留，尤其不能丢失目标自身发射的消息。
其他台的输出作为消除素材使用，不以非目标消息总数充当追台收益。
本次保留原 JTDX 全频段监听及其完整内部输出，没有通过删减基础搜索加速。
原版 WSJT-X/JTDX 源码、候选顺序、内部阈值、AP/OSD 算法及原有测试门槛均不修改。

### 实验配置和可复现性

- 12 个可用 CPU，macOS ARM64，Release、fat LTO、RustFFT；所有 JTDX 路径使用 1 分区。
- 各实验进程串行，计时期间没有编译或其他解码基准。
- 每个配置计时一次。记录整段处理 wall/CPU、首次实际消息回调及全部后台完成时间。
  单次计时用于判断成本量级，不能视为精确性能保证。
- 各组用相同录音、频带 200–3000 Hz、同一随程序发布的 CallDB。
  私有可执行文件目录的 DB SHA-256 为
  `17140773605b0d2a7a5a31458c11384d9efc413bf6288b262b7dd1f391ec32ea`。
  实时 DB 不替换。原有验收另按既有脚本使用私有 2022 校准 DB。
- `syncmin=None`，未试验更低门限。JTDX 高灵敏设置沿用现有构造器，未另行放宽。
- 原 DX 的监听、上下文、奇偶时隙判断、冷启动退避和原恢复任务全部保留。
  新增结果不回写旧上下文，不挤占旧焦点名额，不导入其他分支的可变呼号/AP 状态。
- 消息身份沿用共享 `OutputLedger`：DX 保持 3 Hz、0.3 s 容差。
  未解析/已解析呼号的合并要核对原版完整音调及其余字段，不以短呼号 hash 单独判同。

回调时间按完整 15 秒录音在时隙边界可用来模拟，明确排除声卡、GUI、采集块及系统调度抖动。
原始日志使用了偏早的 14.688 秒起点，分析脚本统一加回 **0.312 秒**；本节数字均已修正。
这只平移模拟回调/完成时标，不改变音频、搜索、消息或实测 wall/CPU。
整段处理秒数不包含等待录音采集的时间。

### 合成素材及真假判定

除原有长录音、`dx_synth_ua3qna.wav` 和 a8d 素材外，新增带逐条真值的素材：

- 使用未修改的原版打包、CRC/LDPC 和 GFSK 生成；打包/解包往返断言成功。
- 高斯白噪声使用固定 xorshift64* 与 Box–Muller；SNR 按 2500 Hz 噪声带宽定义。
  目标标称 SNR 从 −10 到 −27 dB；表中此值不是原版解码器回报的估计 SNR。
- 目标约 1000 Hz，近频干扰在 1025/970 Hz，分别约 −5/−9 dB；另有 1800 Hz 背景台。
- 对照包含目标单独存在、只有干扰、混合信号、纯噪声；目标消息变化并穿插缺席时隙。
- 增益突变将整段接收信号在第 4–8 秒乘以 0.18；目标、干扰和噪声共同受影响。
- 开发/留出噪声种子为 `2026102801`/`2026102809`，各时隙加 `10007*slot`。
  调度和门限固定后才回放留出组。
- 相似信号对照包含目标与其他台通信、别人呼叫目标、目标换频、近似呼号 `BG5ATU`。
- 发现不完整上下文问题后，先固定合法呼号对约束，再用全新种子
  `2026102877`、`2026102879`、`2026102881` 验证，不继续按新验证集调参。

合成样本的正确输出需匹配同一时隙的真实注入消息，且频率/DT 位于约定容差内。
目标呼号出现但内容错误、不完整，也记为错误；不能以“呼号存在”抵消。
实录未标注结果单独列候选，不直接认定为真实增益或已证实误报。
同一生成模型、有限随机种子和理想线性信道不能覆盖真实接收机、过载、传播衰落等情形。

共完成 **109 个串行实验进程**。合成素材有 304 个命名时隙；
去掉成对对照中的重复波形后是 272 个不同波形时隙，
其中 195 个没有任何目标相关报文。共享噪声、重复配置不算独立统计样本。

### 对比方案

| 方案 | 内容 | 用途/结论 |
|---|---|---|
| A | 当前 DX | 逐条目标保留基线 |
| B | A + 独立常规 WSJT-X + JTDX/AGC SWL | 复用 Hybrid 三路观测；两条 JTDX 都是 DX 的 SWL |
| C-J | B + 消除后的 JTDX/noAGC、JTDX/AGC 定向重解 | 初始消融；未保留为默认方案 |
| C | B + 消除后的 JTDX/noAGC 与 WSJT-X 定向重解 | 有合成收益，但有额外深搜长尾和不完整上下文问题 |
| D | B + 受控的 WSJT-X 定向消除恢复 | 最终推荐；保留原 DX 全部 JTDX 任务，不再额外叠加 JTDX 残差深搜 |

新增消除最多选 2 个焦点、3 个种子、做 1 轮。种子来自原监听和新增观测的原版 Regular 输出，
不包含目标相关消息或无法验证的哈希消息；要通过已有的打包/原始音调一致性、
波形支持及能量保护检查。原有 `0.70` 波形支持阈值不改。
种子从焦点附近 ±100 Hz 候选中选择，在独立音频副本上消除；重解窗口为焦点 ±25 Hz。
新增恢复结果不继续作为消除种子，不建立自我强化的强目标状态。

D 还增加两条编排约束：只有两个完整、合法且不同的呼号才给新增 WSJT-X 恢复输入定向 QSO 上下文；
只有目标呼号时清空该新增 worker 的呼号对/网格上下文，使用普通搜索。
新增恢复的显式空呼号 `<>` 不进入正式输出，原始诊断仍保留；正常 `<...>` 不作空呼号处理。
这些限制不改原版代码，也不重筛原 DX 的既有输出。

### 目标消息与实录成本

下表的数量是去重后的目标相关消息，不是全频段总解码数。
D 对 UA3QNA、CU3HN 实测；DD0ZL、RK4FF 没有重复 D，不能把推导值写成实测。

| 目标 | A | B | C | D | 旧目标丢失 |
|---|---|---|---|---|---|
| UA3QNA | 2 | 2 | 2 | 2 | 0 |
| DD0ZL | 5 | 5 | 5 | 未重复 | 0 |
| CU3HN | 14 | 20 | 20 | 20 | 0 |
| RK4FF | 9 | 9 | 9 | 未重复 | 0 |

| 目标 | A wall | B wall | C wall | D wall |
|---|---|---|---|---|
| UA3QNA | 58.82 s | 64.21 s | 64.41 s | 64.07 s |
| DD0ZL | 58.76 s | 62.72 s | 72.67 s | 未重复 |
| CU3HN | 66.69 s | 70.81 s | 83.70 s | 71.43 s |
| RK4FF | 58.61 s | 62.38 s | 69.97 s | 未重复 |

| CU3HN | CPU | 最晚目标回调 | 最后后台完成 |
|---|---|---|---|
| A | 89.37 s | +0.46 s | +7.29 s |
| B | 180.65 s | +2.68 s | +7.63 s |
| C | 194.43 s | +2.69 s | +14.71 s |
| D | 181.79 s | +2.69 s | +7.72 s |

CU3HN 的 B 相比 A 增加 6 条：1 条匹配现有参考表同一时隙/文本，5 条未标注。
参考匹配由 13 增为 14；目标自身发送的行由 7 增为 8，其中新增发送行也仍是未标注候选。
“其他台呼叫目标”不能当成目标自己发射的频点，不能据此改变强目标频率。

| UTC | 新增消息 | 参考状态 |
|---|---|---|
| 140345 | `ON7RED CU3HN HM68` | 未标注候选 |
| 140400 | `CU3HN YC3BMX OI62` | 未标注候选 |
| 140600 | `CU3HN YC3BMX OI62` | 同一时隙/文本已标注 |
| 140630 | `CU3HN YC3BMX OI62` | 未标注候选 |
| 140700 | `CU3HN YC3BMX OI62` | 未标注候选 |
| 140730 | `CU3HN YC3BMX OI62` | 未标注候选 |

`CU3HN YC3BMX OI62` 的参考行只标注了 14:06:00，其余重复时隙不自动升级为确认。
该参考行的 SNR/DT 与原版本次回报不同，本报告沿用既有门槛的时隙/文本匹配口径，
不声称额外完成了独立的波形真值验证。
DD0ZL 的 5 条里有 1 条未标注候选在 A 就已存在；B/C 未新增它。
UA3QNA 保持 2 条且无未标注输出，原先的零额外行要求保持。
RK4FF 的 9 条均保留，哈希显示没有引起目标消息丢失。

C 在 CU3HN 的最慢新增 JTDX worker 到 +14.71 秒才结束，WSJT-X 已在约 +7.71 秒结束。
D 去掉这一额外深搜后，目标集合及实测新增收益保留，长尾回落到上表水平。
原 DX 自己的恢复任务仍可能较晚结束，不能为了让后台数字变短而删掉它们。

### 合成正例：收益与边界

每行有 8 条真实注入目标消息；无干扰两组中 A/B/C 分别都是 5/8、6/8。
对遮挡样本的逐组结果如下，不能只挑开发组有效的那一行：

| 噪声组 | 条件 | A | B | C | D | D 错误输出 |
|---|---|---|---|---|---|---|
| dev | 近频干扰 | 3/8 | 3/8 | 5/8 | 5/8 | 0 |
| dev | 近频干扰＋增益突变 | 2/8 | 2/8 | 5/8 | 5/8 | 0 |
| holdout | 近频干扰 | 4/8 | 4/8 | 4/8 | 4/8 | 0 |
| holdout | 近频干扰＋增益突变 | 2/8 | 2/8 | 4/8 | 4/8 | 0 |

| 两组合计（32 时隙） | A wall | B wall | C wall | D wall |
|---|---|---|---|---|
| 近频干扰 | 36.77 s | 39.34 s | 52.11 s | 43.14 s |
| 近频干扰＋增益突变 | 34.05 s | 36.50 s | 53.28 s | 41.13 s |

D 保留 C 的 7 次额外正确恢复，全部来自消除后的原版 WSJT-X。
它们是不同测试条件中的恢复次数，不是 7 条新的实录报文，也不能推导为固定 dB 灵敏度提升。
输入理想波形与消除参考同源，收益必须再用更多独立实录检查。

| 素材 | 时隙索引 | 额外恢复真值 | 注入 SNR | 回调 |
|---|---|---|---|---|
| dev-mix | 6 | `K1JT BG5ATV RR73` | -22 dB | +1.57 s |
| dev-mix | 8 | `K1JT BG5ATV 73` | -24 dB | +1.88 s |
| dev-mix-pump | 4 | `K1JT BG5ATV R-07` | -20 dB | +1.36 s |
| dev-mix-pump | 6 | `K1JT BG5ATV RR73` | -22 dB | +1.60 s |
| dev-mix-pump | 8 | `K1JT BG5ATV 73` | -24 dB | +2.20 s |
| holdout-mix-pump | 4 | `K1JT BG5ATV R-07` | -20 dB | +1.23 s |
| holdout-mix-pump | 6 | `K1JT BG5ATV RR73` | -22 dB | +1.36 s |

### 误报控制与新验证集

C 在“只给目标呼号、不设置本机呼号”的留出样本中，曾额外输出
`<> BG5ATV -07`，真实注入为 `K1JT BG5ATV R-07`。
该时隙本来有真实目标，问题是新增辅助输出不完整且内容错误；按严格真值计为 **1 次错误**，
不当作目标凭空出现，也不当作灵敏度增益。原有正确消息仍保留。
这次失败保留在原日志中，不能在汇总时隐藏。

D 固定合法上下文入口后重放，原来的 4 条正确消息保留，不再出现该错误。
随后以下新噪声种子样本不再参与调参：

| 新验证素材 | A | B | D |
|---|---|---|---|
| validation-mix | 2/8，错误 0 | 2/8，错误 0 | 2/8，错误 0 |
| validation-blocker | 0/0，错误 0 | 0/0，错误 0 | 0/0，错误 0 |
| validation-confuser | 3/7，错误 0 | 未重复 | 3/7，错误 0 |

开发/留出组的纯噪声、只有干扰、显式已知网格负例，以及近似呼号、目标换频、
与其他台通信的对照，也都保留逐条真值判定。
受控 D 实际重放 10 个配置，其中 8 个合成配置、128 个时隙，
覆盖 65 个不同的无目标相关报文波形；另外 2 个为实录目标配置。
本次 D 的合成正式输出未发现新的错误；实录未标注候选仍没有独立真值。
上文的整个语料覆盖数包含探索版本，不能全算成 D 的验证次数；
有限样本不构成绝对零误报率保证。
呼号存在、前缀与网格相符、多个相同先验的分支一致，均不能单独当作真实性证明。

### 最终开发架构

推荐 **原 DX 必跑路径 + 三路独立观测 + 有条件的一轮 WSJT-X 目标消除恢复**。
不默认增加第二套 JTDX 残差深搜。纯目标呼号场景仍监听、搜索；没有完整呼号对时，
不能强行启用定向 QSO/AP 上下文。自动推断对端呼号属于后续另行验证的策略，本次不启用。

共享只放在编排层：

1. 将 Hybrid 三路的配置、独立会话运行、来源事件及耗时统计提取为公共原版执行组件。
   DX 原有 JTDX 监听是其中一条，不能再启动一条相同的 JTDX 基线。
   Hybrid 保持其策略，DX 保持自己的目标策略，不在 DX 内嵌整个 Hybrid。
2. 复用共享解析器、原始报文去重账本和隔离消除 helper。
   DX 继续用 3 Hz/0.3 s 去重；来源由显式 worker 身份携带，不能从 provenance 猜来源。
3. 原 DX 上下文、焦点、冷启动退避及原恢复任务作为受保护路径。
   新观测只供独立的扩展线索使用；辅助恢复不能锁定强频率、网格和发射奇偶，不能自证身份。
4. 焦点按发送方归属。默认只有焦点尚缺可接受目标发送结果、附近有合格干扰种子时，
   才考虑新增消除；已经恢复的焦点不重复消耗深搜预算，其他焦点仍可继续。
   这一按需策略要在实现时做消融，不能把本次“每个合格焦点都跑”的上限成本当成已优化耗时。
5. 保留独立音频副本、独立呼号/AP/残差状态，一轮、少量种子、最多少量焦点。
   恢复后只调用原版 WSJT-X；完整呼号对与网格只是原版输入，不能改原版门限和列表。
6. 新增任务有单独的开始预算和并发上限，原路径保留资源；不能因新结果改变原焦点配额，
   也不能因新任务让旧任务被 watchdog 跳过。预算只管后续任务是否开始，不中断原版调用，
   不丢弃已经解出的消息。
7. 回调到达即显示，正式输出与后台全部结束分别记录。
   监听目标仍按下一时隙 +2/+3 秒评估；已运行原版调用没有硬实时保证，晚结果仍需输出。
   文件回放保留确定性的任务/种子顺序。实时调度、资源竞争和下一时隙积压要单独验收。
8. 明确保留原版 provenance；未知不冒充 Regular。消除/上下文结果标记为派生恢复，
   JTDX AGC/noAGC 不算两个独立身份确认；报文一致只用于身份去重，不等于真实性证明。

第一版不增加门限档位，不猜测大量报告/网格，不把新增候选自动提升为强上下文，
不把当前有限素材的最佳结果宣称为所有真实接收条件下的最佳组合。

### 实现及验收顺序

- 先提取公共执行/观测组件，确认 Hybrid 三路与 DX 原路径输出逐条不退化。
- 接入 DX 两条互补观测、统一账本和显式来源，完成所有旧目标与参考匹配对比。
- 接入受控 WSJT-X 单轮消除、合法上下文入口、空呼号诊断隔离和独立预算。
- 对上面的真实目标、7 次合成正确恢复、单呼号失败回归、新种子负例做本机优化构建验收。
  不能用新增数量抵消丢失的旧目标；若按需调度损失已验证收益，需调整编排后再验收。
- 增加长时间监听与稠密频段压力回放，记录回调 P95/最差值、CPU、后台长尾、队列和下一时隙积压。
  声卡/GUI 实机时延另测，不能用本次离线模拟替代。
- 不修改原有验收断言。新反例增加约束，不重写旧真值、不降低旧数量/误报门槛。

本次已在本机通过原有 `test_dx_profile_long_ua3qna`、
`test_dx_profile_synthetic_reproducible`、`test_dx_profile_synthetic_ua3qna` 三项手动门槛，
以及 `test_dx_profile_a8d_fixture`。原测试源码和 82 个镜像原版源码及 8 个受保护输入哈希不变。
这些是现有生产实现的回归检查；候选方案另由本节逐条实验和真值检查支撑，不混为已上线验收。

复现材料包括 `tasks.json`、`environment.json`、各轮二进制/源码指纹、
三组 `spec.tsv`/`truth.tsv`、逐条 `RAW/ROW/BASE/NATIVE/SIC/WORKER` 日志、
`analysis.json`、原有门槛日志和独立实验源代码。
实验目录为 `/tmp/ft8rs-dx-lab`，复现包为该目录下的 `dx-experiment-replay.tar.gz`。
原始失败轮次与受控新轮次分开保存，计时修正保留在分析脚本和元数据中。

## 实现后本机验证（2026-10-28）

以下为生产代码开发后的实测，与上面的开发前原型数字分开。
实现为原 DX 全部任务 + 原监听复用的三路观测 + 条件触发的一轮受控 WSJT-X 消除。
没有增加 JTDX 残差深搜，原版镜像与既有验收源码均未修改。

### 测量方法与硬约束

- Release/RustFFT、12 个可用 CPU、默认 1 分区、相同录音/配置/随程序发布的 CallDB。
- 保留旧版二进制，9 个关键配置重新测旧版时间；开发版用直接引用当前生产 core 的回放程序。
  其他控制素材逐条比对已有旧版基线。31 个实时配置、5 个文件配置均完成。
- 共 45 个串行回放进程。各计时一次，编译、其他解码测量和验收测试不与计时并行。
  wall 包含整段离线处理，不包含等待音频采集；CPU 是进程总 CPU，不是单线程用时。
- 开发版从完整 15 秒音频可用的时隙边界计回调。旧版原始日志的 14.688 秒起点仍只对
  旧数据加回 0.312 秒；开发版没有这项补偿。声卡/GUI/采集块抖动不包含在内。
- 以原始报文、频率、DT 和时隙逐条比对旧目标，新增不能抵消丢失。
  所有开发版配置的旧目标损失为 0；受保护原任务的预算跳过为 0，新增 worker 错误为 0。

### 实录消息与耗时

| 目标 | 旧消息 | 开发版消息 | 旧目标丢失 | 整段 wall：旧 → 新 | 最晚新回调 | 新后台结束 |
|---|---|---|---|---|---|---|
| UA3QNA | 2 | 2 | 0 | 59.31 → 63.82 s | +1.63 s | +4.15 s |
| CU3HN | 14 | 20 | 0 | 67.01 → 71.36 s | +2.69 s | +7.66 s |
| DD0ZL | 5 | 5 | 0 | 59.28 → 63.80 s | +0.55 s | +4.14 s |
| RK4FF | 9 | 9 | 0 | 59.08 → 63.27 s | +0.12 s | +4.15 s |

CU3HN 的 CPU 从 89.80 秒增至 182.80 秒。
三路观测用更多 CPU 换取互补结果，本机 wall 增幅较小，不应据此声称 CPU 成本下降。
当前最高实录后台尾部来自保留的原 DX 任务；目标消息即时回调，不等后台全部完成。

CU3HN 保留原型的 6 条新增：参考同一时隙/文本匹配增加 1 条，另外 5 条仍未标注。
参考匹配由 13 增为 14，目标自身发送行由 7 增为 8，新增发送行也仍未标注。
未标注行不当作已确认真实增益。DD0ZL 原有未标注候选、RK4FF 哈希相关目标都保留；
UA3QNA 仍为 2 条，无未标注行，既有零额外行条件保持。

### 七次已验证恢复与条件调度

每个配置有 8 条注入目标消息；下列 wall 均来自本轮新测的旧版/开发版。

| 素材 | 旧正确 | 开发版正确 | 错误 | wall：旧 → 新 | 最晚回调 | 新增恢复次数 | 已恢复焦点跳过 |
|---|---|---|---|---|---|---|---|
| dev-mix | 3/8 | 5/8 | 0 | 17.94 → 22.50 s | +1.91 s | 5 | 3 |
| dev-mix-pump | 2/8 | 5/8 | 0 | 16.56 → 19.87 s | +2.20 s | 6 | 2 |
| holdout-mix | 4/8 | 4/8 | 0 | 18.76 → 22.06 s | +0.50 s | 5 | 3 |
| holdout-mix-pump | 2/8 | 4/8 | 0 | 17.48 → 20.69 s | +1.37 s | 6 | 2 |

相对旧 DX 保留 7 次额外正确恢复：R-07、RR73、73 等报文均匹配注入真值。
这不是 7 条新的实录报文，也不代表普遍增加固定 dB 的灵敏度。
四组只启动 5～6 次新增恢复，少于每个合格焦点均启动的原型 8 次；已验证收益未损失。
五组文件/实时模式的目标信号集合一致，包括单呼号反例。
这些素材的实时预算未压掉收益；一般输入中，实时开始预算与文件无预算策略仍可能产生不同的
新增集合，不能把有限对照外推为所有录音的等价性。

### 假阳性与缺失上下文

开发版实际回放 25 个合成实时配置、400 个时隙；
去除重复配置/成对控制中的重复波形后覆盖 272 个不同波形，其中 195 个没有目标相关报文。
全部合成正式输出匹配真值，错误为 0；有限样本不能证明实际无线电误报率为零。
本机呼号为空、已知网格、目标缺席、近似呼号、不同通信对端、换频、增益突变及新种子均覆盖。

此前不完整呼号对导致的 `<> BG5ATV -07` 没有再出现；其旧版正确目标消息仍保留。
当前限制同时包含按需跳过已恢复焦点，以及给新增 WSJT-X 输入完整合法呼号对的入口约束。
这些是编排限制，未修改原版 AP/CRC/LDPC 判决或同步门限；正常 `<...>` 不按空呼号排除。
实录中未标注候选依然没有独立真值，不能把合成零错误替换成实录零误报结论。

### 本机检查与交付状态

- 213 项常规 workspace 测试通过，包括 DX 三路目标保留/重配置检查。
- 2 项新增优化构建手动波形门槛通过：七次已验证恢复，以及单呼号/目标缺席反例。
- 原有 7 项核心 Release 手动验收全部通过，包括 DX、Hybrid、固定 4 分区的 JTDX 430/431。
- 引擎长录音 Hybrid actor 验收通过，检查共享调用边界没有丢失 Hybrid 独有消息。
- 严格 workspace/all-targets Clippy、fmt、diff 检查通过，CLI/GUI Release 构建通过。
- 82 个原版镜像源码、8 个受保护输入和原验收测试源码的哈希不变。

生产实现落在共享 `cooperative/observation.rs`、DX 协调器和 `dx/augmentation.rs`；
不新增门限档位，不替换原上下文，不以恢复结果制造新的独立证据。
`DxSlotDiagnostics` 将原任务、三路观察、条件跳过、额外开始预算和错误分开记录。

回放数据、前后对比、二进制/输入指纹及验收日志在 `/tmp/ft8rs-dx-development`。
复现数据包为该目录的 `dx-implementation-validation.tar.gz`。未执行 CI，未自动提交或推送。
