# ACCEL — 解码加速可行性研究与成果

研究 FT8 解码"硬件加速"（特殊 CPU 指令集 / 厂商数学库 / GPU）的可行性，并在
**不破坏与上游 WSJT-X/JTDX 字节级对齐、不降灵敏度**的前提下落地优化。

**最终成果：整体解码时间 −30%**（长样本 118.2s → 82.3s / 19 slot），全程 bit-exact，
基线全绿，纯 stable / 无 unsafe / 无运行时探测 / 全平台一致。详见 §5–§6。

---

## 1. 不可逾越的前提

来自项目硬规则（`feedback-no-touch-decode-core`、`perf-decode-decisions`）：

1. **字节级对齐**：wsjtx 21/21 & 424/424、jtdx 20/20 & 430/431 基线必须 byte-identical。
2. **不降灵敏度**，不改 `lib_wsjtx`/`lib_jtdx` 的算法语义。
3. **内核解码循环是串行设计**（H1 已否决并行候选）：`ft8b` 逐候选 subtract-as-you-go
   会改残差 `dd8`，并行化会改变哪些候选能解出 → 改灵敏度 / 破对齐。

**根本张力**：GPU、浮点 SIMD、MKL/Accelerate、`target-cpu=native` 几乎都会改变浮点
舍入（FMA 收缩、不同 FFT 分解、向量化超越函数近似），一旦结果变 bit 就破坏基线。

**唯一安全的加速判据**（最后全部优化都遵守）：
- ✅ **整数 / 位运算**：恒精确（GF(2) XOR、popcount）。
- ✅ **逐元素映射、迭代间无累加**：Rust 默认不做 FMA 收缩，向量化逐位不变。
- ✅ **算法层去冗余 / 记忆化**：用完全相同的运算与顺序复用结果 → 逐位相同。
- ❌ **重排单个浮点归约的加法顺序**：改舍入，禁止。

不满足以上判据的方案只能走 opt-in feature + 独立后端 + 默认关闭（如现有 `fftw`），
本研究未采用。

---

## 2. 可替换点

项目里唯一现成的数值后端边界是 FFT：`four2a.rs::four2a_c2c / four2a_r2c`，已用
`#[cfg(feature="fftw")]` 在 rustfft / FFTW 间切换。其余热点无此边界，需自建。

---

## 3. P0 仪表与方法（commit `5e968e0`）

为避免凭空估算，先做逐阶段计时：

- 新增 cargo feature `profiling` + `src/decode/profile.rs`：无锁原子逐阶段累加器、
  RAII `scope()` 守卫。探针只包在**调用点**（不动算法体）。
- feature **关闭时** `scope` 是零大小、无 `Drop` 的 no-op → 编译为空、零开销、不改
  浮点 → 默认构建与基线字节级一致。
- 复现命令见文末附录。

---

## 4. 实测：热点分布

样本 `tests/ft8/230208_140300.wav`，profile=jtdx，19 slot，118.2s（~6.2s/slot），
Release（fat LTO）+ Apple Silicon (arm64)。

| 阶段 | 占 slot | 性质 |
|---|--:|---|
| **sync8** | 44.2% | FFT + 2D 相关（浮点） |
| **ft8b**（含子项） | 55.8% | 逐候选 |
| └ **osd** | 40.2% | GF(2) 位矩阵 + 深搜浮点归约 |
| └ ldpc-bp | 2.6% | tanh/atanh |
| └ subtract | 3.1% | FFT |
| └ ft8b 自身 | ~10% | symbol metrics / FFT-32 |

**OSD 回退率 98.8%**：BP 几乎从不独立成功，真正干活的是 OSD。

**sync8 子阶段细分**（后续加 SyncSpectra/Sync2d/SyncExtract 探针实测）：

| sync8 子阶段 | %slot | 说明 |
|---|--:|---|
| └ spectra (FFT) | 9.3% | 比预期小得多 |
| └ **sync2d (2D相关)** | **37.3%** | sync8 的真正大头，≈OSD |
| └ extract | 0.2% | 忽略 |

→ **关键修正**：sync8 的成本不在 FFT，而在 `compute_sync2d` 的 2D 相关。

---

## 5. 已落地成果（均 bit-exact，基线全绿）

### P2.0 — OSD 行 XOR 向量化　✅（commit `10d030d`）
新增 `ft8v2/gf2.rs`（ft8rs 自有、非镜像）的 `gf2_row_xor`，把 OSD 两处 GF(2) 行
XOR 热点（高斯消元 + `mrbencode91_into`）委托给非别名 slice 内核，让 LLVM 自动
向量化。osd174_91.rs 算法结构不变。
- **结果**：osd 47557→39793 ms（**−16.3%**），整体 −8%。

### 路径 B — sync2d 窗口和去冗余　✅（commit `0cc4fd1`）
`compute_sync2d` 内层成本几乎全在滑动窗口和 `sum_s`/`sum_s_stride`。因
`k = j+jstrt+nssy·n`，**同一 `(i,k)` 在不同 `(j,n)` 下被重算约 10 次**。两条路径
（plain 用 `sum_s` 17元素窗；AGC `compute_sync2d_agc` 用 `sum_s_stride` step=2）各
预计算一次窗口和表 `rs[i][k]`，**用完全相同的求和顺序 → 逐位相同**，cell 内改查表。
- **结果**：sync2d 40851→**9799 ms（−76%）**，sync8 −59%，整体再 −25%。

### 累计
相对最初 P0（118243 ms）：**整体解码 −30%**（P2.0 + 路径B）。

---

## 6. 尝试并否决

### P2.1 — u64 位打包 OSD 高斯消元　❌ 实测零收益，已回退（doc `e96e672`）
实现了 `BitMatrix`（pack/swap_cols/xor_row）并把高斯消元改为打包路径，bit-exact
（属性测试 + 基线均绿）。但 release 实测 **osd 42587→42596/42707 ms（0~0.3%，纯
噪声）**。原因：P2.0 已把行 XOR 自动向量化；**高斯消元根本不是 OSD 的瓶颈**——
OSD 的 ~42.6s 主要花在深搜的**浮点加权和归约**（`xor_weight_sum_*`、
`error_weight_sum`，REDUCE，不可 bit-exact 重排）。已回退，不留死代码。
**教训：OSD 的 bit-exact 加速已到顶**；再快只能动算法/灵敏度（红线）。

### P3 / sync8 浮点 SIMD　❌ owner 否决
不接受任何改变浮点舍入、需要"容差等效"的方案。sync8 浮点 SIMD / 批量 FFT /
`target-cpu=native` 全部排除。

### MKL / IPP　❌
x86-only，本机 arm64 不可用；等价物 Apple Accelerate，但 FFT 非独立大头，收益撑
不起依赖复杂度。

### GPU（CUDA/Metal/wgpu）　❌
OSD 是分支密集的串行位/归约搜索（对 GPU 敌对）；逐候选 FFT 太小（n=32/256），
kernel 启动延迟吃光收益；解码循环串行（H1）不能批量候选；Apple Silicon 只有
Metal；且破字节对齐。

---

## 7b. lib_wsjtx 路径（标准 WSJT-X 解码器）

上面 §3–§6 针对 lib_jtdx（高灵敏度）。lib_wsjtx 是独立的标准解码器（passes/候选
更少、OSD `ndeep=2` 无 npre2 box 搜索），热点分布**完全不同**。加同款探针实测
（长样本，`--profile wsjtx`，commit `d25d72a`）：

| stage | %slot | total | calls |
|---|--:|--:|--:|
| slot | 100% | 65933 ms | 19（~3.47s/slot）|
| sync8 | 3.5% | 2336 ms | 114 |
| └ spectra (FFT) | 1.8% | 1186 ms | — |
| └ sync2d | **0.9%** | 574 ms | — |
| ft8b | 60.8% | 40075 ms | 42784 |
| └ **osd** | **50.2%** | 33080 ms | 44656 |
| └ ldpc-bp | 2.8% | 1862 ms | 22939 |
| subtract | 7.4% | 4862 ms | 697 |

OSD 回退率 194.7%（每次 bp 失败触发 nosd=2 次 osd）。

**结论**：
- **OSD 一家独大 50.2%**，sync 几乎可忽略（3.5%，sync2d 仅 0.9%）。
- **路径B（sync2d 去冗余）在 wsjtx 不值得做**——sync2d 只占 0.9%。
- **wsjtx-P2.0（`gf2_row_xor` 迁移到 `lib_wsjtx/ft8/osd174_91.rs`）　✅ DONE**
  （commit `e31d073`）：把 `gf2_row_xor` 提升为共享模块 `crate::decode::gf2`
  （仿 `crate::decode::profile`，两个 mirror 共用），wsjtx OSD 的高斯消元 + mrbencode
  两处行 XOR 委托给它。bit-exact。
  - **结果**：osd 33080→**23847 ms（−27.9%）**，osd 单次 740→534 µs，调用数 44656
    不变；**slot 65933→56351 ms（−14.5%）**。比 jtdx P2.0（osd −16%）更猛——wsjtx
    OSD 调用 44656 次、每次跑全消元，且无 npre2 box 稀释。
  - **对齐**：wsjtx 19/424 + jtdx 20/20 & 430/431 复验绿；gf2 单测绿。
- subtract 7.4%（逐元素复乘 MAP，Tier-1，可向量化但 FFT 占大头，收益小）；
  bp 2.8% 跳过；sync2d 0.9% 不做。wsjtx 的 bit-exact 加速到此基本到顶。

---

## 7c. OSD 内部剖析（探针实测，决定后续开发）

OSD 是两个解码器的最大单项（jtdx 40~52%，wsjtx 42~50%）。曾用临时子探针
（OsdElim/Encode/Dist/Box + 机器级 OsdCount/OsdArr）量过其内部构成，**结论确凿后
探针已撤除**（它们处于 per-pattern 热路径、开启时严重失真），数据留档于此。

**jtdx OSD 内部**（osd=52.2% slot，40.4s）：

| 子项 | 占 OSD | 说明 |
|---|--:|---|
| osd-box（npre2 box 构建）| 23.5% | boxit91 哈希配对（ndeep=3 特有）|
| osd-enc（mrbencode）| ~12% | 重编码 |
| osd-dist（加权和距离）| ~4.5% | 浮点归约 |
| osd-elim（高斯消元）| ~4% | |
| **其余（搜索机器）** | **~56%** | 图样枚举/数组拷贝/HashMap/e2 更新 |

**wsjtx OSD 内部**（osd=42.5% slot，22.8s；ndeep=2，无 box）：

| 子项 | 占 OSD | 说明 |
|---|--:|---|
| osd-elim | ~7% | |
| osd-enc | ~6% | |
| osd-dist | **0.2%** | 几乎为零 |
| osd-box | 0% | 确认无 npre2 |
| **其余（搜索机器）** | **~87%** | 同上 |

**wsjtx 机器再细分**（per-n1 探针，含测量地板高估，绝对 ms vs 真实 OSD 22.8s）：

| 机器子项 | ≈占 OSD | bit-exact 可优化 |
|---|--:|---|
| osd-arr（`e2.copy` + `e2 ^= genmrb行`）| ~35%（高估）| ✅ 打包 u64 → 2-word XOR/copy |
| osd-cnt（`filter().take(40).count()`）| ~17%（高估）| ✅ `popcount(word & mask)` |
| 其余（nextpat91 枚举/mi/me/setup/控制流）| ~另一半 | ❌ 只能换算法 |

**关键结论**：
1. **OSD 没有"SIMD 算术"的优化空间**：dist（曾设想的 SIMD 目标）仅 0.2~4.5%，废案。
   OSD 的成本是**搜索结构本身**（机器），不是紧致数值循环。
2. **但约 20~30% 的 OSD 是 bit-exact 可打包的**（osd-arr + osd-cnt，扣除探针地板后
   的保守估计）：把 `e2/e2sub` 打包成 `u64` + genmrb 校验列打包 → 行 XOR/拷贝
   collapse；`filter().count()` → `popcount`。结果逐位相同（GF(2)/整数），不丢解码。
3. 这是**后续开发的目标**（暂称 OSD-PACK）：潜在 OSD −20~30%（整体 wsjtx ~−10% /
   jtdx ~−8%），bit-exact、零正确性风险（基线兜底），但是镜像 OSD 内循环的 fiddly
   重写，且有 P2.1 式"被另一半机器 + 打包开销吃掉收益"的不确定性。
4. 再大的提速只能**换算法**（减少调用/降搜索深度/砍 npre2）= 拿灵敏度换，对 DX 追台
   不利，须开独立实验 profile + 独立验收。

---

## 7. 剩余 backlog（bit-exact，owner 决定是否做）

边际收益均已不大（两个最大头的便宜部分已吃完）：

| 项 | 预计收益 | 难度 | 说明 |
|---|---|---|---|
| 路径 A：sync8 频谱跨 band 复用 | 净 3–5% | 中 | spectra 仅 9.3%，且 dd8 被 subtract 频繁改写，命中率有限 |
| 路径 C：sync2d 跨频点 SIMD | 中 | 高 | 每 cell 独立，按频点 i 向量化、每 lane 内求和顺序不变 → bit-exact；需手写 SIMD（gather） |
| Tier-1 MAP 自动向量化 | 个位数 % | 低 | `normalize_tone_spectra` 除法、`compute_symbol_spectra` 幅值、`subtractft8`/`ft8_downsample` 逐元素循环 |

**已彻底否决，勿再提**：P2.1 / SIMD-OSD（实测无效）、sync8 浮点 SIMD、BP/tanh
（2.6%）、GPU、MKL/IPP、独立 r2c（H2）。

---

## 里程碑

```
P0       ✅ 仪表 + 实测（5e968e0）
P2.0     ✅ gf2_row_xor 向量化 jtdx OSD（osd −16%, 整体 −8%）   bit-exact
路径B    ✅ sync2d 窗口和去冗余（sync2d −76%, 整体再 −25%）     bit-exact
         —— jtdx 累计相对 P0：整体解码 −30%
wsjtx-P2.0 ✅ gf2_row_xor wsjtx OSD（osd −27.9%, 整体 −14.5%）  bit-exact
OSD剖析  ✅ OSD 内部探测完成（§7c）：dist 仅 0.2~4.5%、机器主导；
            探索性 per-pattern 探针已撤、数据留档；保留 6 个粗粒度阶段探针
OSD-PACK-1 ✅ wsjtx 机器自动向量化（e2 XOR→gf2_row_xor，count→无分支 sum）
            受控 A/B(AC,min×3)：osd 24411→23041ms（−5.6%），整体 wsjtx −2.8%，
            bit-exact（53e4a55）。组内波动 <1%，信号真实。
OSD-PACK-3 ✅ 移植到 jtdx OSD（A/B AC,min×3）：osd 40989→40554ms（−1.1%）
            bit-exact（5fdfa21）。比 wsjtx 小——jtdx OSD 被 npre2 box 主导，
            order-1 机器只是小头。
OSD-PACK-2 ❌ 显式 u64 打包：不做。phase 1 已吃掉"标量→向量化"那一跳，
            打包的边际（count→popcount ~1-3%）撑不起 fiddly 重写 + P2.1 风险。
→ OSD bit-exact 自动向量化到此收官：wsjtx OSD −5.6% / jtdx OSD −1.1%。
  再大的 OSD 提速只能动 npre2 box / 算法（换灵敏度，对 DX 不利，须独立 profile）。
P2.1     ❌ u64 打包 OSD 高斯消元：实测零收益 → 已回退（消元非瓶颈）
路径A/C  ⏸ sync8 频谱复用（净 3–5%）/ sync2d 跨频点 SIMD（更难）
P3       ❌ sync8 浮点 SIMD（破对齐，owner 否决）
```

---

## 附 A：性能数据汇总（所有实测，留档）

**测量条件**：Apple Silicon 笔记本，fat-LTO release，长样本
`tests/ft8/230208_140300.wav`（19 slot），曾用 feature-gated `profiling` 探针
逐阶段计时。**全流程解码计时对热节流/电池敏感（同代码跨时间可差 ±25%）**；
里程碑 delta 一律用**受控 A/B**（`git stash` 切换 → 重建 → 各跑 3 次取 min，AC 电）。
**探针子系统已于收官后整体移除**（见附 B），数据归档于此。

### jtdx（`--profile jtdx`）

| 里程碑 | slot(ms) | osd(ms) | 关键 delta |
|---|--:|--:|---|
| P0 baseline | 118243 | 47557 (40%) | — |
| +P2.0（OSD gf2_row_xor）| 108705 | 39793 | osd **−16%**, 整体 −8% |
| +路径B（sync2d 记忆化）| 82308 | ~40000 | sync2d **−76%**, 累计 **−30%** |
| +OSD-PACK（A/B）| — | 40554 vs 40989 | osd **−1.1%** |

- sync8 内部：spectra(FFT) 9.3% / **sync2d 37.3%** / extract 0.2%（占 slot）
- OSD 内部：**box 23.5%** / enc 12% / dist 4.5% / elim 4% / 机器 ~56%（占 OSD）

### wsjtx（`--profile wsjtx`）

| 里程碑 | slot(ms) | osd(ms) | 关键 delta |
|---|--:|--:|---|
| original | 65933 | 33080 (50%) | — |
| +wsjtx-P2.0（OSD gf2_row_xor）| 56351 | 23847 | osd **−27.9%**, 整体 **−14.5%** |
| +OSD-PACK（A/B）| 54215 vs 55785 | 23041 vs 24411 | osd **−5.6%**, 整体 −2.8% |

- sync8 内部：spectra 1.8% / sync2d **0.9%** / extract 0%（→ 路径B 在 wsjtx 无用）
- OSD 内部：elim 7% / enc 6% / **dist 0.2%** / box 0% / **机器 ~87%**（占 OSD）
- 机器再细分（探针地板高估）：osd-arr(e2 XOR/copy) ~35% / osd-cnt(count) ~17%

### 累计（全部 bit-exact，对齐零改动）

| 解码器 | 优化栈 | 整体 |
|---|---|--:|
| **lib_jtdx** | P2.0 + 路径B + OSD-PACK | **~−31%** |
| **lib_wsjtx** | wsjtx-P2.0 + OSD-PACK | **~−17%** |

## 附 B：再测量方法（探针已移除）

profiling 子系统（`profiling` feature + `profile.rs` + 探针）收官后已移除，热路径
回到与上游对齐的形态。若后续要再量 OSD-PACK-2 等：**建议建独立微基准**（Dev-0：
dump 真实 `(llr, apmask, ndeep)` 入参，criterion 紧循环跑 `osd_decode174_91`），
比全流程解码稳定得多，免受热节流干扰。对齐复验仍用基线：
```
cargo test --profile fast -p ft8rs-core test_jtdx_profile -- --ignored   # 2 passed
cargo test --profile fast -p ft8rs-core test_stream_decode                # 3 passed (wsjtx 19/424)
```
