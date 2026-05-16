# FT8 解码技术分析报告

> 项目：ft8rs vs WSJT-X FT8 解码器技术差异分析与改进方案
>
> 目标：解码全部 20 条消息（当前 16/20），解码时间 < 180s

### 源码验证

本地 WSJT-X 源码位于 `wsjtx/` 目录，版本与 saitohirga/WSJT-X GitHub 镜像一致。

已验证的关键文件（`wsjtx/lib/ft8/` 下）：

| 文件 | 行数 | 功能 |
|------|------|------|
| `ft8_decode.f90` | 297 | 主解码入口，多pass策略 |
| `ft8b.f90` | 516 | 单信号精细解码 + AP解码 |
| `sync8.f90` | 147 | 粗同步搜索（2D相关网格） |
| `sync8d.f90` | 39 | Costas精细同步（复相关） |
| `subtractft8.f90` | 117 | 信号减法（复基带+LPF+精炼） |
| `ft8_downsample.f90` | 42 | 宽频提取+下采样 |
| `ft8c.f90` | ~100+ | a7模式信号重检 |
| `ft8apset.f90` | — | AP符号表初始化 |
| `ft8_a7.f90` | — | a7历史数据管理 |

**确认：本地源码与网上获取的参考代码一致，以下分析基于本地源码。**

---

## 1. 现状评估

### 1.1 测试基线

| 测试文件 | 当前 ft8rs (depth=3) | 目标 |
|----------|---------------------|------|
| 210703_133430.wav | 16/20 条, 1.82s | 20/20 条, <180s |
| 190227_155815.wav | 27/30+ 条, 1.89s | 数据充足可作对比 |

**关键发现：ft8rs 与 ft8ts 参考实现解码结果完全一致（同为 16/20）。缺失的 4 条连 ft8ts 也解不出来，只有 Fortran WSJT-X (default mode) 能解全部 20 条。** 参见 ft8ts README benchmark 表。

缺失消息：`K1BZM EA3CJ JN01`, `KD2UGC F6GCP R-23`, `WA2FZW DL5AXX RR73`, `CQ EA2BFM IN83`

当前性能：解码速度快（远超 180s 目标），但灵敏度不足（差 4 条消息）。

### 1.2 ft8rs 架构概览

```
decode() 
  ├── copy/resample → dd (float64)
  ├── sync8()           ← 粗同步：滑动FFT功率谱 + Costas 相关搜索
  │     └── fft_complex() → 功率谱 2D 网格
  │     └── candidates[] (freq, dt, sync 评分)
  ├── [Pass 1]:
  │     for each candidate:
  │        ft8b()        ← 精细解码
  │          ├── ft8_downsample()      ← 宽频提取 → cd0 (3200点)
  │          ├── find_best_time_offset() ← Costas 时偏搜索(+/-10 samples)
  │          ├── find_best_frequency_shift() ← 频偏搜索(+/-2.5Hz, 11档)
  │          ├── ft8_downsample()      ← 精确频率重采样
  │          ├── refine_time_offset()  ← 精细时偏修正(+/-4 samples)
  │          ├── extract_soft_symbols() ← 软符号提取 (79 symbols × 8 tones)
  │          ├── passes_sync_gate()    ← Costas 同步门控 (≥7/21 hits)
  │          ├── build_bit_metrics()   ← 比特度量(4种:1/2/3-符号组合)
  │          ├── try_decode_passes()   ← BP+OSD 解码 (maxosd=2)
  │          └── unpack77() + estimate_snr()
  │        subtract_decoded_signal()   ← 已解码信号从残差中减去
  ├── [Pass 2, depth≥3]:
  │     重复上述过程 (使用残差信号)
  └── 输出 decoded[]
```

---

## 2. WSJT-X 解码器关键差异分析

参阅 WSJT-X v2.7.0 源码 (`lib/ft8_decode.f90`, `lib/ft8/ft8b.f90`, `lib/ft8/sync8.f90`, `lib/ft8/subtractft8.f90`)

### 2.1 多级解码深度策略 (核心差异)

**WSJT-X 的 `ndepth` 参数含义（源码 `ft8_decode.f90:168-194`）：**

```fortran
! ndepth=1: 1 pass, bp
! ndepth=2: subtraction, 3 passes, bp+osd (no subtract refinement)
! ndepth=3: subtraction, 3 passes, bp+osd
npass=3
if(ndepth.eq.1) npass=2
```

| 阶段 | ndepth=1 | ndepth=2 | ndepth=3 |
|------|----------|----------|----------|
| 外层 pass 数 | 2 | 3 | 3 |
| 粗同步 syncmin | 2.0 (nzhsym=41) → 1.3 | **1.6** | **1.3** |
| Pass 1 ndeep | 1 | 2 | 2 (降级) |
| Pass 2 ndeep | — | 2 | 3 |
| Pass 3 ndeep | — | 2 | 3 |
| 内层 BP/AP  | BP only | BP+OSD | BP+OSD |
| 减法精炼(lrefinedt) | 无 | **无** | **有** |

**ft8rs 的 depth 映射：**
- depth=1: 单 pass, maxosd=-1 (仅 BP), syncmin=1.2
- depth=2: 单 pass, maxosd=0 (仅 BP), syncmin=1.2  
- depth=3: 双 pass, maxosd=2 (BP+OSD[2]), syncmin=1.2

⚠️ **核心差异总结：**
1. ft8rs 最多 2 个减法 pass，WSJT-X 有 3 个
2. ft8rs syncmin=1.2 固定，WSJT-X 按 ndepth 动态调整（1.6→1.3）
3. ft8rs 无减法频偏精炼（lrefinedt）
4. ft8rs 无内层 AP 解码（iaptype 1-6）
5. ft8rs 无 a7 历史复用

### 2.2 渐进式解码 (Progressive Decoding)

**WSJT-X 特性：** 在 15s 帧的 3 个关键时间点进行增量解码：

| nzhsym | 含义 | 操作 |
|--------|------|------|
| 41 | 前 ~10.25s | 初解码：高灵敏度(syncmin=2.0)，筛选强信号 |
| 47 | 前 ~11.75s | 减法后：从 dd1 状态继续解码 |
| 50 | 完整 15s | 最终解码：使用完整数据，对剩余未减信号再做减法 |

**ft8rs 缺失：** 一次性处理全部 15s 数据，无渐进式解码阶段。

**影响：** 渐进式解码允许 WSJT-X 在更早时间点以更高阈值（syncmin=2.0）检测强信号，然后通过减法在后续阶段中发现弱信号。

### 2.3 AP (A Priori) 解码

**WSJT-X 特性（`ft8b.f90:265-400+`）：**

AP 解码在 ft8b 内部以额外的 LDPC 解码 pass 实现：

```fortran
npasses = 4 + nappasses(nQSOProgress)  ! 基础4次 + 根据QSO状态
if(.not.lapcqonly) then
   iaptype = naptypes(nQSOProgress, ipass-4)
endif
```

**iaptype 定义（6种消息类型）：**

| iaptype | 消息结构 | AP 比特数 | 应用场景 |
|---------|----------|----------|----------|
| 1 | CQ ??? ??? | 29+3=32 | 主叫方 |
| 2 | MyCall ??? ??? | 29+3=32 | 应答方 Tx1/Tx2 |
| 3 | MyCall DxCall ??? | 58+3=61 | 通联建立 Tx3 |
| 4 | MyCall DxCall RRR | 77 | 确认 Tx4 |
| 5 | MyCall DxCall 73 | 77 | 结束 Tx4 |
| 6 | MyCall DxCall RR73 | 77 | 确认+结束 Tx4 |

**Contest 模式特殊约束（ncontest 0-8）：**

```fortran
! ncontest=0 : NONE (标准 AP)
! ncontest=1 : NA_VHF
! ncontest=2 : EU_VHF
! ncontest=3 : FIELD DAY
! ncontest=4 : RTTY
! ncontest=5 : WW_DIGI
! ncontest=6 : FOX (无 AP)
! ncontest=7 : HOUND (FOX 呼号hash + 受限频率)
! ncontest=8 : ARRL_DIGI
```

AP 工作方式：
1. 将已知比特的 `apmask` 置为 1（指示已在 LLR 中给了强先验）
2. 将对应比特的 LLR 设为 `±apmag`（幅度约 5-10 的强先验值）
3. Contest 模式下使用特定的比特模式（mcq/mcqru/mcqfd 等掩码）
4. 若已有呼号信息（apsym 不为空），直接注入 LLR

**关键代码示例（CQ模式 AP）：**
```fortran
if(iaptype.eq.1) then
   apmask(1:29)=1
   llrz(1:29)=apmag*mcq(1:29)     ! CQ 消息前29位固定模式
   apmask(75:77)=1
   llrz(75:76)=apmag*(-1)          ! i3=0 (Type 0)
   llrz(77)=apmag*(+1)             ! n3=0 (标准)
endif
```

**ft8rs 缺失：** 完全无 AP 解码支持，LDPC 解码器不接收 apmask 先验。

**影响量化：** CQ 模式 AP 可将 29+3=32 比特从未知变为已知，有效码率从 77/174≈0.44 提升到 (77-32)/(174-32)=45/142≈0.32，理论上可提升 2-4 dB 解码灵敏度。

### 2.4 a7 历史复用 + nagain 重检

**WSJT-X 特性：**

1. **a7 历史复用：** 保存上一时隙的解码结果，在下一时隙中通过 `ft8c.f90` 重新解码同频/同时偏的信号
```fortran
call ft8_a7d(dd, newdat, call_1, call_2, grid4, xdt, f1, ...)
```

2. **nagain 重检：** 在 nzhsym=50 时，对 QSO 频率 ±20Hz 范围内再次精细搜索
```fortran
if(nzhsym.eq.50 .and. nagain) then
   dd=iwave
   ifa=nfqso-20
   ifb=nfqso+20
endif
```
   - 使用完整 15s 原始数据（无减法污染）
   - 窄带搜索（仅 nfqso 附近），降低虚警
   - 调用 `ft8b` 时 `nagain=.true.`，触发更 agressive 的 SNR 计算（使用 xbase 归一化）

**ft8rs 缺失：** 无跨时隙信息复用，无 nagain 窄带重检。

**影响：** 对重叠在强信号下、频偏相近的弱信号，无法利用先验信息辅助解码。nagain 模式在实时操作中可额外挽救 1-3 条信号。

### 2.5 减法策略差异

**WSJT-X 减法特点（`subtractft8.f90`）：**

1. **复基带幅度估计：**
   - 生成完整参考波形 `cref = exp(j*(2π*f0*t + φ(t)))`
   - IQ 混频: `camp(i) = dd(j) * CONJG(cref(i))`
   - LPF 提取复幅度: `cfilt = LPF[camp]` （cos²窗，NFILT=4000 点）
   - 端部修正 (endcorrection) 消除滤波器瞬态

2. **频偏/时偏精炼 (lrefinedt)：**
   - 在 ±90 sample 范围内搜索最佳时偏
   - 判据：减法后残留频谱能量最小化
   - 仅在 ndepth=3 时启用

3. **信号重构与减法：**
   - `dd(j) = dd(j) - 2.0 * REAL(cref(i) * cfilt(i))`
   - 2.0 因子用于双边谱到实信号的幅度映射

4. **渐进式减法：**
   - nzhsym=47：对已解码的早期信号进行减法（dt-0.5 < 0.396s）
   - nzhsym=50：对剩余未减信号的完整帧减法

**ft8rs 减法（简化版）：**

```rust
// 简单的最小二乘幅度估计
let amp = estimate_amplitude(&waveform, &residual);
let i_component = amp * SUBTRACTION_GAIN * (2π*f*t + phase).cos();
let q_component = amp * SUBTRACTION_GAIN * (2π*f*t + phase + SUBTRACTION_PHASE_SHIFT).sin();
residual[i] -= (i_component + q_component) as f64;
```

- 使用固定 `SUBTRACTION_GAIN=0.95` 和 `SUBTRACTION_PHASE_SHIFT=π/2`
- 无 LPF 幅度提取（残差较大）
- 无频偏时偏精炼
- 无端部修正

**差异影响：** WSJT-X 的复基带幅度估计 + LPF 可以更准确地估计信号幅度随时间的变化，减法残差显著更小。频偏精炼可避免因频率估计不准确导致的减法不彻底。

### 2.6 LDPC 解码策略

**ft8rs：**
- 4 种比特度量（bmeta, bmetb, bmetc, bmetd）混合使用
- `scalefac = 2.83`
- BP 迭代固定 30 次
- OSD order-2 (maxosd=2)

**WSJT-X：**
- 支持 AP 掩码约束的 BP + OSD 解码
- 根据 `nQSOProgress`（通联状态）选择解码策略
- Contest 模式激活特定的比特模式匹配

**差异影响：** WSJT-X 在有已知呼号信息时，LDPC 解码门槛大幅降低，可解码 SNR 低 2-4 dB 的信号。

### 2.7 同步阈值策略

| 实现 | syncmin |
|------|---------|
| ft8rs | 固定 1.2 |
| WSJT-X ndepth=1/2 | 1.6 |
| WSJT-X ndepth=3 | 1.3 (最终 pass) |
| WSJT-X nzhsym=41 | 2.0 (早期高阈值) |

ft8rs 的固定低阈值 (1.2) 会产生更多虚假候选，但可能漏掉的部分是由其他因素（解码而非同步）引起的。

### 2.8 信号检测方面的细微差异

**WSJT-X 额外特性：**
1. **Baseline 归一化：** 使用滑动窗口频谱基线 `sbase`，对宽带噪声进行频率相关归一化
2. **xbase 用于解码：** `xbase = 10^(0.1*(sbase(f1/3.125)-40))` 在解码时利用基线信息
3. **Fox 模式：** 特殊的 Fox 呼号哈希解码

**ft8rs：**
- 计算 baseline 但实际解码中未使用 `_sbase`（在 sync8 中计算，传给 ft8b 但未参与核心解码逻辑）
- 不支持的 Fox/Contest 模式

### 2.9 Contest 模式

WSJT-X 支持多种 contest 模式（ARRL FD, ARRL RTTY, WW Digi 等），contest 模式下使用额外的 AP 约束：
- 呼号格式已知约束
- 网格位置格式约束
- 特殊消息类型约束

这些约束可有效减少 LDPC 解码的搜索空间，提高弱信号解码能力。

---

## 3. 缺失的 4 条消息分析

### 3.1 可能原因推断

基于 WSJT-X 代码分析，缺失的 4 条消息可能属于以下情况：

1. **弱信号 (SNR < -20dB)：** 
   - 需要 AP 解码才能解出
   - 当前 BP+OSD 在无先验信息下信噪比门槛约为 -19dB

2. **被强信号掩盖的信号：**
   - 减法不够精确，残差中留有干扰
   - 需要频偏精炼和更精确的减法幅度估计
   - 需要渐进式减法（nzhsym=47 时减去短时突发信号）

3. **频偏/时偏接近的重叠信号：**
   - 同步检测在一个候选附近有多个信号，只有最强被解出
   - 需要在解码后继续搜索附近频率（ft8rs 的 dedup 过滤掉 4Hz/0.04s 以内的候选）

4. **特殊消息类型：**
   - 非标准消息格式（如 i3=4 的 Type 2 呼号哈希消息）
   - 需要 Type 2 hashtable 解码支持

### 3.2 验证方法

使用 WSJT-X 处理同文件获得参考输出，与 ft8rs 结果对比：
- 确认缺失消息的 SNR、dt、freq
- 分析缺失消息的信号特征（是否与已解码强信号频率重叠）

---

## 4. 改进方案

### 4.1 优先级 P0: 解码增强（预期收益最大）

#### 4.1.1 降低同步门控 (sync_gate)

```rust
// 当前: min_costas_hits = 7 (depth=3: 6)
// 修改为与 WSJT-X 一致：min_costas_hits = 6 或更低
const MIN_COSTAS_HITS_DEEP: usize = 5;  // 放宽同步门控
```

WSJT-X 的 `sync8d` 不使用严格的 Costas 门控，而是依赖总相关积分值。

#### 4.1.2 增加解码 pass 数 (3-pass subtraction)

```rust
// 当前: max_passes = 2 (depth=3)
// 改为: max_passes = 3 (depth=3)
const MAX_DECODE_PASSES_DEPTH3: usize = 3;
```

对应 WSJT-X 的 3-pass 策略：
- Pass 1: ndeep=2, BP+OSD(0), 解码所有可解信号
- Pass 2: ndeep=2, BP+OSD(0), 使用残差解码
- Pass 3: ndeep=3, BP+OSD(2), 最后深度搜索

#### 4.1.3 优化减法策略

**频偏精炼:**
```rust
// 在减法前增加 GFSK 匹配滤波器细调
fn refine_for_subtraction(waveform: &[f32], residual: &[f64], 
    f_est: f64, dt_est: f64) -> (f64, f64, f64) {
    // 搜索最佳频率/时间/相位偏移
    // 返回 (refined_freq, refined_dt, refined_gain)
}
```

**渐进式减法:**
```rust
// 分割 15s 帧为 3 段（对应 WSJT-X nzhsym=41,47,50）
// 在每段中独立进行解码-减法循环
fn progressive_decode(dd: &[f64]) -> Vec<DecodedMessage> {
    // Phase 1: 前 10.25s, syncmin=2.0, 仅解码强信号并减法
    // Phase 2: 前 11.75s, syncmin=1.6, 基于残差继续解码
    // Phase 3: 完整 15s,  syncmin=1.3, 最终深度解码
}
```

#### 4.1.4 减法后再次搜索相同频率

WSJT-X 在减法后使用 `nagain` 模式在 qso 频率 (+/-20Hz) 范围内重搜：

```rust
// 在 subtraction pass 后，对已解码频率附近再次精细搜索
fn recheck_near_decoded(cx_re: &[f64], cx_im: &[f64], 
    decoded_freqs: &[f64]) -> Vec<Candidate> {
    // 对每个已解码频率 +/-20Hz 范围重新运行 sync8d
}
```

### 4.2 优先级 P1: AP 解码（中等收益）

#### 4.2.1 单文件内 AP 解码

对于单个 WAV 文件（无跨时隙上下文），实现有限 AP 解码：

```rust
// Non-coherent AP: 使用 CQ 消息结构约束
fn apply_cq_ap_mask(llr: &mut [f64], apmask: &mut [i8]) {
    // CQ 消息固定比特约束:
    // - i3[74:76] = 0 (Type 0)
    // - n3[71:73] = 0 (标准消息)
    // 约束这些比特的 LLR 为强先验
}

fn apply_grid_ap_mask(llr: &mut [f64], apmask: &mut [i8]) {
    // 网格定位器的合法范围约束
}
```

#### 4.2.2 Contest 模式 AP

```rust
fn ft8apset(mycall: &str, hiscall: &str, ncontest: usize) -> ApSym {
    // 根据 contest 类型设置 AP 符号掩码
    // 参考 WSJT-X ft8apset.f90
}
```

### 4.3 优先级 P2: 信号检测增强（较小收益）

#### 4.3.1 降低候选过滤条条件

```rust
// 当前: 频率差 <4Hz 且 时差<0.04s 的去重过滤
// 可能过于激进，考虑放宽:
// - 频率去重窗口从 4Hz → 3Hz
// - 时差去重窗口从 0.04s → 0.06s
// - 保留更多候选给 ft8b 精细解码
```

#### 4.3.2 Costas 同步相关性增强

```rust
// 使用与 WSJT-X 一致的频率偏移模板 (11档 +/-2.5Hz)
// 但增加更精细的搜索步长
// 考虑增加到 21 档 (+/-5Hz, 步长 0.5Hz)
```

#### 4.3.3 频谱基线在比特度量中的应用

```rust
// 将 sbase (频谱基线) 引入比特 LLR 计算
// 类似 WSJT-X 使用 xbase 对频率依赖性噪声进行归一化
fn apply_baseline_to_llr(llr: &mut [f64], sbase: &[f64], freq_bin: usize) {
    let xbase = 10.0_f64.powf(0.1 * (sbase[freq_bin] - 40.0));
    for i in 0..llr.len() {
        llr[i] /= xbase;
    }
}
```

### 4.4 优先级 P3: LDPC 解码器增强

#### 4.4.1 增加 BP 迭代次数

```rust
// 从固定 30 次 → 自适应: 最多 50 次
// 在 SNR 临界情况下更多的迭代可能收敛
const MAX_BP_ITERATIONS: usize = 50;
```

#### 4.4.2 多重解码尝试

```rust
// 对每个候选尝试多种比特度量组合
fn try_multiple_metrics(workspace: &DecodeWorkspace) -> Option<DecodeResult> {
    // 除 bmeta/bmetb/bmetc/bmetd 外
    // 尝试加权组合: w1*bmeta + w2*bmetb 等
}
```

#### 4.4.3 nharderrors 阈值调整

```rust
// 当前: nharderrors <= 36
// WSJT-X 使用 nharderrors + dmin 组合判断
// 对弱信号放宽到 40
const MAX_HARD_ERRORS: usize = 40;
```

### 4.5 性能保障 (保持 <180s)

| 优化项 | 预期时间增加 | 风险 |
|--------|-------------|------|
| 3-pass subtraction | +0.5s/pass ≈ +1.5s | 低 |
| 渐进式解码 | +2-3s (更多 FFT) | 低 |
| AP 解码 | +0.1-0.2s/pass | 极低 |
| 频偏精炼减法 | +0.2s/signal | 低 |
| 更多 BP 迭代 | +0.1s | 极低 |
| 更多候选 | +0.3-0.5s | 中 |

**总预期增加：5-10s，仍在 180s 目标内大幅冗余。**

---

## 5. 改造步骤

### Phase 1: 快速改进（1-2天）

1. **增加第 3 个解码 pass**
   - 修改 `MAX_DECODE_PASSES_DEPTH3 = 3`
   - 确保每个 pass 独立更新残差

2. **放宽同步门控**
   - `passes_sync_gate` 从 7→5（depth=3 时从 6→5）

3. **降低 nharderrors 阈值**
   - 从 36 → 40

4. **增加减法幅度精度**
   - 移除 `SUBTRACTION_GAIN` 固定缩放
   - 实现与 WSJT-X 对齐的 I/Q 幅度估计

5. **测试、对比 WSJT-X 输出**

### Phase 2: 解码精度提升（3-5天）

6. **实现频偏精炼减法 (lrefinedt)**
   - 参考 `subtractft8.f90` 的逻辑

7. **实现渐进式解码**
   - 分 3 段时间窗口处理

8. **实现基线归一化 LLR**

9. **优化候选过滤策略**

### Phase 3: AP 解码（3-5天）

10. **实现 CQ/Grid AP 掩码**
11. **实现 contest 模式 AP**
12. **实现 a7 历史复用（如果处理多帧文件）**

### Phase 4: 精细优化（2-3天）

13. **LDPC 解码器性能调优**
14. **全参数扫描优化**
15. **回归测试 + benchmark**

---

## 6. 参考文件清单

| 文件 | 说明 |
|------|------|
| `wsjtx/lib/ft8_decode.f90` | WSJT-X 主解码入口 |
| `wsjtx/lib/ft8/ft8b.f90` | WSJT-X 单信号精细解码 |
| `wsjtx/lib/ft8/sync8.f90` | WSJT-X 粗同步检测 |
| `wsjtx/lib/ft8/sync8d.f90` | WSJT-X Costas 相关同步 |
| `wsjtx/lib/ft8/subtractft8.f90` | WSJT-X 信号减法 |
| `wsjtx/lib/ft8/ft8_downsample.f90` | WSJT-X 下采样 |
| `ft8rs/src/ft8/decode.rs` | ft8rs 解码主逻辑 |
| `ft8rs/src/util/decode174_91.rs` | ft8rs LDPC 解码器 |
| `ft8rs/tests/ft8/210703_133430.wav` | 测试文件 (目标 20/20) |

---

## 7. 总结

ft8rs 当前的核心问题不是架构缺陷，而是缺少 WSJT-X 的以下关键特性：

1. **3-pass 减法** vs 当前 2-pass（影响最大）
2. **AP 解码** 的完全缺失（影响中等）
3. **减法精度** 不足（无频偏精炼）  
4. **同步/解码门控** 偏保守
5. **基线归一化** 未用于 LLR 计算

按 Phase 1→4 逐步实施，预期可达到 20/20 条消息目标，解码时间控制在 30s 以内。
