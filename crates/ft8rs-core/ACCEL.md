# ACCEL — 解码硬件加速可行性报告与开发计划

本文档记录 FT8 解码"硬件加速"（特殊 CPU 指令集 / 厂商数学库 / GPU）的可行性
研究：背景约束、可替换点、P0 实测统计、结论，以及据此制定的开发计划。

加速模块的总目标：**可与普通实现完全替换、默认关闭、普通电脑照常可用**，且
不得破坏与上游 WSJT-X/JTDX 的字节级对齐、不得降低灵敏度。

---

## 1. 不可逾越的前提

来自项目硬规则（`feedback-no-touch-decode-core`、`perf-decode-decisions`）：

1. **字节级对齐**：wsjtx 21/21 & 424/424、jtdx 20/20 & 430/431 基线必须保持
   byte-identical。
2. **不降灵敏度**，不碰 `lib_wsjtx`/`lib_jtdx` 的算法语义，新东西放编排层 /
   后端边界。
3. **内核解码循环是串行设计**（H1 已否决并行候选）：`ft8b` 逐候选
   subtract-as-you-go 会改残差 `dd8`，并行化会改变哪些候选能解出 → 改灵敏度 /
   破对齐。

**根本张力**：GPU、不同 SIMD 浮点数学、MKL/Accelerate 几乎都会改变浮点舍入
（FMA 收缩、不同 FFT 分解、向量化超越函数近似），一旦结果变 bit 就破坏
byte-identical 基线。因此任何加速后端只能走和现有 `fftw` feature 一样的模式：
opt-in、独立后端、默认关闭。**唯一例外是精确（整数/位）运算**——它们的 SIMD
版可做到 bit-exact，从而保住对齐（见结论中的 OSD）。

---

## 2. 现成的可替换点

项目里唯一干净的数值后端边界是 FFT：

- `four2a.rs::four2a_c2c(re, im, isign)` 与 `four2a.rs::four2a_r2c(re, im)`
- 已用 `#[cfg(feature="fftw")]` 在 rustfft / FFTW 间编译期切换，签名是
  `&mut [f64]` 实/虚部 slice，干净可换。

这就是"模块可完全替换、普通电脑照常能用"的模板。其余热点目前没有这样的边界，
需自建后端 trait。

---

## 3. P0 仪表与方法

为避免凭空估算，先做逐阶段计时（commit `5e968e0`，gui 分支）：

- 新增 cargo feature `profiling` + `src/decode/profile.rs`：无锁原子的逐阶段
  累加器（纳秒 + 调用次数）、RAII `scope()` 守卫。
- 探针只包在**调用点**（不动算法体）：`sync8`、`ft8b`、`ldpc-bp`、`osd`、
  `subtract`、整 slot —— 位于 `lib_jtdx/mod.rs`、`ft8b/mod.rs`、
  `ft8b/regular.rs`。
- feature **关闭时** `scope` 是零大小、无 `Drop` 的 no-op → 编译为空、零开销、
  不改浮点 → 默认构建与基线字节级一致。
- 运行方式：
  ```
  cargo build --release -p ft8rs-cli --features profiling
  ./target/release/ft8rs file <wav> --profile jtdx --start-time YYMMDD_HHMMSS
  ```
  CLI 的 `profiling` feature 透传到 core，解码结束打印报告。

**对齐复验**：feature 关闭下 `cargo test --profile fast test_jtdx_profile -- --ignored`
→ `2 passed`（20/20 与 430/431 floor 均通过）。

---

## 4. 实测结果

样本 `tests/ft8/230208_140300.wav`，profile=jtdx，19 个 slot，总计 118.2 s
（约 6.2 s/slot）。Release（fat LTO）+ Apple Silicon (arm64)。

| 阶段 | 占 slot | total | calls | avg | 性质 |
|---|--:|--:|--:|--:|---|
| slot（分母） | 100% | 118.2 s | 19 | 6.2 s | 整槽 |
| **sync8** | **44.2%** | 52.3 s | 1026 | 50.9 ms | FFT + 2D 相关（浮点） |
| **ft8b**（含子项） | 55.8% | 65.9 s | 25755 | 2.56 ms | 逐候选 |
| └ **osd** | **40.2%** | 47.6 s | 45752 | 1.04 ms | GF(2) 位矩阵（整数） |
| └ ldpc-bp | 2.6% | 3.1 s | 46330 | 66 µs | tanh/atanh（浮点） |
| └ subtract | 3.1% | 3.6 s | 484 | 7.46 ms | FFT（浮点） |
| └ ft8b 自身 | ~10% | — | — | — | symbol metrics / FFT-32 |

**OSD 回退率 98.8%**：45752/46330 次 BP 尝试失败后落到 OSD；OSD 单次 1040 µs，
是 BP（66 µs）的约 16×。即 BP 几乎从不独立成功，真正干活的是 OSD。

---

## 5. 结论

1. **两个热点 = sync8 (44%) + OSD (40%) = 84%**，其余全是噪声。
2. **OSD 是首选加速目标，且最特殊**：它是 GF(2) 位矩阵运算（高斯消元 + 有序
   统计搜索），**整数/位运算、不是浮点**。因此：
   - MKL / FFT / GPU-FFT 对它**完全无关**；
   - 正确武器是 **SIMD 位并行**（u64 / AVX2 / NEON 上的 XOR + popcount）；
   - GF(2) 运算**精确**，SIMD 版可做到 **bit-exact → 保住字节级对齐**（浮点
     SIMD 做不到）。ROI 最高 + 对齐风险最低。
3. **BP / tanh 加速彻底出局**：BP 仅 2.6%，向量化超越函数不值得。
4. **sync8（FFT + 相关）是浮点那一半**：SIMD / 批量 FFT 可做，但改舍入 → 需
   独立"等效"验收（浮点容差），列为第二优先。
5. **GPU 被数据否决**：OSD 是分支密集的串行位搜索（对 GPU 极不友好）；逐候选
   FFT 太小（n=32/256），kernel 启动延迟吃光收益；解码循环串行（H1）不能批量
   候选；Apple Silicon 只有 Metal 非 CUDA；且破字节对齐。
6. **Intel MKL/IPP 出局**：x86-only，在本机（arm64）无法使用；跨平台等价物是
   Apple Accelerate，但 FFT 非独立大头（被 sync8/ft8b-self 吸收），收益撑不起
   依赖复杂度。
7. 此前 deferred 的 rustfft r2c（H2）维持低价值结论：FFT 散落在 sync8/ft8b
   内部，不是独立 >10% 的项目。

---

## 6. 三条路线总评

| 路线 | 可行性 | 说明 |
|---|---|---|
| **CPU SIMD（位并行 OSD）** | ✅ 强烈推荐 | bit-exact 可保对齐；命中 40% 热点；运行时探测 + 标量回退 → 普通机照常用 |
| **CPU SIMD（浮点 sync8/FFT）** | ⚠️ 次选 | 命中 44% 但改舍入，需独立等效验收 |
| **Intel MKL / IPP** | ❌ | x86-only，本机 arm64 不可用 |
| **GPU（CUDA/Metal/wgpu）** | ❌ | OSD 对 GPU 敌对、小 FFT 受启动延迟、循环串行、破对齐 |

---

## 7. 开发计划

总原则：每个加速后端 = opt-in feature + 运行时特性探测 + **标量回退永远在**
（普通电脑零影响）。bit-exact 路径并入字节级基线验收；浮点路径走独立等效验收。

### P2（主线）—— bit-exact SIMD OSD 后端　【最高优先】
目标：把 40% 的 OSD 做成位并行 SIMD，且与现实现**逐位相同**，从而无需放宽
任何基线。

- **P2.0 建后端边界**　✅ DONE：新增 `ft8v2/gf2.rs`（ft8rs 自有、非镜像），把
  OSD 两处行 XOR 热点（高斯消元 + `mrbencode91_into`）委托给 `gf2_row_xor`
  （非别名 slice，LLVM 自动向量化）。osd174_91.rs 算法结构不变，bit-exact。
  - **结果**（长样本，对比 P0）：osd 47557→39793 ms（**−16.3%**），osd 单次
    1039→870 µs，调用次数 45752 不变；slot 118243→108705 ms（−8.1%）。
  - **对齐**：jtdx 20/20 & 430/431 复验绿；gf2 单测绿。纯 stable / 无 unsafe /
    无运行时探测 / 全平台一致。
#### P2.1（详细计划）—— u64 位打包 GF(2) 矩阵，仍 bit-exact　【工作量大，待批】

动机与诚实的 ROI 评估：P2.0 已让编译器对**字节** XOR 自动向量化，所以 P2.1 的
增量收益主要来自**内存带宽**而非计算——高斯消元反复流式扫 `genmrb`（k×n=91×174
字节）做 k×k 次行 XOR，是内存受限的。把行从"1 bit/字节"打包成 `u64`（每行
N=174 bit → W=⌈174/64⌉=3 word），带宽降约 7×、行 XOR 3 word/次。预计 osd 再降
约 **10–20%**（整体约 +4–7%）；`mrbencode` 增益较小（已被 P2.0 向量化）。**若
P2.1b 实测 <5%，回退仅打包 genmrb（高斯消元）不打包 codeword。**

数据结构（放 `ft8v2/gf2.rs`，ft8rs 自有、非镜像）：
```
const W: usize = (N + 63) / 64;          // = 3
struct BitMatrix { rows: usize, words: Vec<u64> }   // rows*W，行优先
  fn get(r, c) -> u8 / test_bit(r, c) -> bool         // (word >> bit) & 1
  fn set(r, c, b)
  fn swap_cols(c1, c2)            // 跨所有行交换两 bit 列（逐行位翻转）
  fn copy_row_words(r) -> [u64; W]  // 取出 pivot 行，解别名
  fn xor_row(dst_r, &pivot_words)   // dst 行 ^= pivot（W 个 word XOR）
```

实施步骤：
- **P2.1a** 在 gf2.rs 加 `BitMatrix` + 单测：随机 0/1 矩阵下，get/set/swap_cols/
  xor_row 与现有 `Vec<u8>` 行优先参考实现**逐位相同**。
- **P2.1b** 把 osd174_91.rs 的 `genmrb: Vec<u8>` 换成 `BitMatrix`：
  - 构建（列重排，现 L23–29）→ `set`
  - 主元搜索 `genmrb[id_row+icol]==1`（现 L36/L49）→ `test_bit`
  - 列交换 `genmrb.swap(...)`（现 L40）→ `swap_cols`
  - 行消元（现已是 `gf2_row_xor`）→ `copy_row_words` + `xor_row`
  - `boxit91_pattern` / `e2 ^= genmrb[...]`（现 L141/L306）→ `test_bit`
  - `mrbencode91_into`：codeword 也用 W-word 累加；weight-sum 处按位展开
- **P2.1c** 验收：随机 LLR/apmask 下 OSD 输出码字与 P2.0 **逐位相同**的属性测试；
  jtdx 20/20 & 430/431 基线绿；`--features profiling` 复测 osd 降幅并记录。
- **风险**：①列交换的位操作 + 主元行序必须与原实现完全一致（否则码字变）；
  ②codeword 打包后 weight-sum 的展开顺序要保持。逐位对拍 + 基线为双重护栏。
- **范围控制**：纯 stable、无 unsafe、无运行时探测、全平台一致（与 P2.0 同档）。
  显式 SIMD intrinsics（原 P2.2）**暂不做**——u64 打包已让编译器向量化，先看够不够。

### P3（原 sync8 浮点 SIMD）—— ❌ 不做（会改浮点 → 破对齐）
owner 决定：不接受任何改变浮点舍入、需要"容差等效"的方案。sync8 的浮点 SIMD /
批量 FFT / `target-cpu=native` 全部排除。sync8 的提速只能走下方 bit-exact 路径。

### sync8 内部细分（实测，长样本）　✅ DONE
profile 加 SyncSpectra/Sync2d/SyncExtract 三个子阶段后实测：

| sync8 子阶段 | %slot | total | 说明 |
|---|--:|--:|---|
| └spectra (FFT) | **9.3%** | 10173 ms | 比预期小得多 |
| └sync2d (2D相关) | **37.3%** | 40851 ms | 真正的大头，≈OSD(36.6%) |
| └extract | 0.2% | 202 ms | 可忽略 |

**结论修正**：sync8 的成本**不在 FFT，而在 `compute_sync2d` 的 2D 相关**。
原"频谱复用"假设（以为 FFT 是大头）被推翻——见下。

### 路径 A：sync8 频谱跨 band 复用　【降级为小项】
`compute_symbol_spectra`（spectra）只依赖 `dd8`+mode，与 band 无关，理论可缓存。
但实测它仅占 **9.3%**，且 `dd8` 在 band 循环里被 `subtractft8` 频繁改写（484 次 /
1026 sync8 调用），命中率有限 → 净收益预计仅 **3–5%**。低优先。

### 路径 B：sync2d 的 sum_s 去冗余　【强候选，bit-exact，纯标量】
**已核实**：`compute_sync2d` 内层成本几乎全在 `sum_s(&s, i, i+16, k)`（16 元素滑窗
和）。但 `k = j + jstrt + nssy·n`，**同一 `(i,k)` 在不同 `(j,n)` 下被重复计算约 6
次**（j 跨 ~150 dt、n 跨 ~16）。
→ 预计算 `rs[i][k] = sum_s(i, i+16, k)`（用**完全相同的求和顺序** → 逐位相同），
cell 内改查表。`t0a += rs[i][k] - s[...]` 的运算与累加顺序不变 → **bit-exact**。
纯标量、可移植、无 intrinsics，直击 37.3% 的大头，预计收益**远大于路径 A 与 P2.1**。
注意：plain 路径用 `sum_s`，AGC 路径（`compute_sync2d_agc`）用 `sum_s_stride`
（step=2）——按实际热路径分别做同款记忆化。
属性：sync2d 输出与现实现逐位相同 + jtdx 20/20 & 430/431 绿 + profiling 量降幅。

### 路径 C：sync2d 跨频点 SIMD（Tier 2）　【备选，更难】
若路径 B 后仍想压：每 cell 独立，按频点 i 向量化、每 lane 内求和顺序不变 →
bit-exact。但需手写 SIMD（滑窗 gather），复杂度高。建议路径 B 之后再评估。

### bit-exact SIMD 机会清单（全管线扫描结果，不动对齐）
判据：**INT**（整数/位）恒精确；**MAP**（逐元素、迭代间无累加）向量化精确
（Rust 默认不做 FMA 收缩，逐元素 IEEE 运算不变）；跨"独立输出"并行的归约精确；
**重排单个归约的加法顺序 ❌**（改舍入）。

**上限说明**：两个最大阶段已无便宜 SIMD 余量——sync8 的成本在 FFT
（four2a/rustfft 库码，已优化），OSD 的成本在高斯消元（P2.0 已向量化）+ 加权和
**归约**（不可 bit-exact 重排）。故剩余 SIMD 收益受限，均为中小项。

- **Tier 1（MAP，便宜，靠自动向量化，全平台，bit-exact）**——做法同 P2.0：
  干净 slice、消边界检查/别名让 LLVM 自动向量化，必要时 runtime `#[target_feature]`
  MAP 内核。候选：
  - `ft8b/mod.rs::normalize_tone_spectra` 逐元素除法（~7×79×7/候选，条件触发）
  - `sync8.rs::compute_symbol_spectra` 的 `sqrt(re²+im²)`（NH1×NHSYM/次，≈该函数 6%）
  - `subtractft8.rs` 三个逐元素复乘/相减（151k/180k/151k，上限 subtract 3%）
  - `ft8_downsample.rs` 窗/移位/平均 MAP 循环（多为 3200/次）
  - 预计合计整体个位数 %。
- **Tier 2（归约，bit-exact 但需"跨输出并行"重写，工作量大）**：
  - `sync8.rs::compute_sync2d` 2D 相关——sync8 内除 FFT 外的大头。每 cell 为固定
    几项之和，**按频点 i（独立输出）向量化、每 lane 内求和顺序不变 → bit-exact**。
    收益可观但需手写 SIMD（gather、变长内和）。
- **Tier 3（不做）**：单归约加权和（osd `xor_weight_sum_*`、bp 变/校验节点更新）
  无法 bit-exact 重排、个体价值低；bp 的 tanh（2.6%）。

排序：SYNC 频谱复用（算法，最大）> Tier 2 sync2d（大但难）> Tier 1 MAP（便宜小赢）
> Tier 3 不做。落地前建议加细分 profiling 量准 Tier 1/2 真实占比再投入。

### 不做（已被数据否决，除非 owner 重新签字）
- sync8/任何浮点 SIMD（破对齐，owner 否决）
- BP/tanh 向量化（2.6%）
- GPU（CUDA/Metal/wgpu）
- Intel MKL/IPP（本机 arm64 不可用）
- 独立 r2c 优化（H2，低价值）

### 里程碑顺序
```
P0    ✅ 仪表 + 实测（done, 5e968e0）
P2.0  ✅ gf2_row_xor 边界 + 自动向量化（osd −16%, 整体 −8%, bit-exact）
SYNCP ✅ sync8 子阶段 profiling：spectra 9.3% / sync2d 37.3% / extract 0.2%
路径B ▶ sync2d sum_s 去冗余（bit-exact 纯标量，攻 37.3%，强候选）
P2.1  ⏸ u64 位打包 OSD（再压 osd ~10–20%；bit-exact；工作量大，待批）
路径A ⏸ sync8 频谱复用（实测仅 9.3%，净 3–5%，低优先）
路径C ⏸ sync2d 跨频点 SIMD（更难，路径B 之后再评估）
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

# 对齐复验（feature 关闭 = no-op 路径）
cargo test --profile fast -p ft8rs-core test_jtdx_profile -- --ignored
```
