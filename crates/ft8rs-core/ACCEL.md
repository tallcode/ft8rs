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
P0    ✅ 仪表 + 实测（5e968e0）
P2.0  ✅ gf2_row_xor 向量化（osd −16%, 整体 −8%）          bit-exact
SYNCP ✅ sync8 子阶段 profiling：spectra 9.3% / sync2d 37.3%
路径B ✅ sync2d 窗口和去冗余（sync2d −76%, 整体再 −25%）   bit-exact
      —— 累计相对 P0：整体解码 −30%
P2.1  ❌ u64 打包 OSD：实测零收益 → 已回退
路径A ⏸ sync8 频谱复用（净 3–5%，低优先）
路径C ⏸ sync2d 跨频点 SIMD（更难）
P3    ❌ sync8 浮点 SIMD（破对齐，owner 否决）
```

---

## 附：复现实验命令

```
# 构建带探针的 release
cargo build --release -p ft8rs-cli --features profiling

# 长样本逐阶段报告
./target/release/ft8rs file crates/ft8rs-core/tests/ft8/230208_140300.wav \
  --profile jtdx --start-time 230208_140300 >/dev/null

# 对齐复验（feature 关闭 = no-op 路径；应 2 passed）
cargo test --profile fast -p ft8rs-core test_jtdx_profile -- --ignored
```
