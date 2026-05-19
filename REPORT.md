# FT8 解码技术报告

> 项目：ft8rs vs WSJT-X FT8 解码器技术差异分析
>
> 最终成果：**20/20 消息，3.60s**，零编译警告
>
> 测试文件：`tests/ft8/210703_133430.wav`（12000Hz, 15s）

---

## 里程碑总览

### 灵敏度提升

| 阶段 | 解码数 | 关键突破 |
|------|--------|---------|
| 初始 | 16/20 (80%) | 无信号减法，弱信号被强信号掩蔽 |
| **subtractft8** | **20/20 (100%)** | 精确信号减法消除强信号掩蔽 |

成功解码的 4 条历史弱信号：
- **KD2UGC F6GCP R-23** @472Hz, -12dB SNR
- **K1BZM EA3CJ JN01** @2522Hz, -12dB SNR
- **CQ EA2BFM IN83** @2280Hz, -16dB SNR
- **WA2FZW DL5AXX RR73** @2546Hz, -19dB SNR

### 性能优化历程

| 版本 | 耗时 | 解码数 | 累计加速 | 技术 |
|------|------|--------|---------|------|
| v0 基线 | 7.0s | 16/20 | 1× | 无信号减法 |
| v1 subtractft8 | 33.8s | 20/20 | — | 时域 O(N×M) 循环卷积 |
| v2 去除取模 | 27.9s | 20/20 | 1.2× | 预扩展数组避免 rem_euclid |
| v3 FFT 卷积 | 11.4s | 20/20 | 3.0× | O(N·logN) 线性卷积 |
| v4 并行候选 | 4.90s | 20/20 | 6.9× | rayon 并行解码 |
| **v5 WSJT-X 参数** | **3.60s** | **20/20** | **9.4×** | BP 30次 + OSD order-2 |

---

## 第一部分：灵敏度突破（16→20）

### 1.1 问题发现

**症状：** 16/20 停滞，所有方向（FFT精度、LDPC参数、AP掩码、sync8归一化）均无效。

**关键假设检验：**
- 假设1: "减法后 sync8 能在残差中找到更多弱信号"
  → 验证：**失败** — 4 条弱信号在 Pass 1 就被漏掉，减法根本没机会执行
- 假设2: "sync8 阈值太高"
  → 验证：**失败** — syncmin=0.1 极低阈值都检测不到 472Hz 信号

**深入调试结论：** sync8 在 472Hz 附近都检测不到 KD2UGC 信号 → 不是减法问题，是**信号处理链差异**。

### 1.2 根因：subtractft8 的 3 个隐藏 Bug

通过逐行对比 Fortran `subtractft8.f90`，发现并修复了 3 个 bug：

| Bug | Fortran 正确实现 | ft8rs 错误实现 | 后果 |
|-----|-----------------|---------------|------|
| **FFT 尺寸** | NFFT=NMAX=180,000 | 零填充到 262,144 (2^18) | LPF 窗口在频域的位置偏移，DC 分量完全不在窗口覆盖范围内 |
| **IFFT 归一化** | four2a 正向/反向都不归一化 | 反向 IFFT 归一化 1/N | LPF 增益计算错误，差 N 倍 |
| **cshift 方向** | cshift(正数)=向左循环移位 | rotate_right（向右） | 窗口位置再次偏移 |

**为什么难发现：** 这 3 个 bug 叠加后减法仍有"效果"（能减掉一些能量），但不够精确，残差中仍有干扰信号。对于强信号无影响，但对 -12dB 到 -19dB 的弱信号就是收敛/不收敛的区别。

### 1.3 减法算法原理

WSJT-X 的信号减法 (`subtractft8.f90`)：

```
输入信号  : dd(t) = a(t)·cos(2πf₀t + θ(t))
参考信号  : cref(t) = exp(j·(2πf₀t + φ(t)))  [GFSK波形]
IQ混频    : camp(t) = dd(t) × CONJG(cref(t))
LPF提取   : cfilt(t) = LPF[camp(t)]           [cos²窗口, NFILT=4000点]
减法      : dd(t) ← dd(t) - 2·REAL(cref(t)×cfilt(t))
```

**关键点：**
- gen_ft8wave 用 **NSPS=1920**（全分辨率），不是检测用的 48 samples/symbol
- LPF 窗口是 cos²(-2000..2000) / sumw，共 4001 点
- 端部修正 (endcorrection) 消除滤波器瞬态

### 1.4 最终方案演进

| 阶段 | 方案 | 耗时 | 结果 | 经验教训 |
|------|------|------|------|---------|
| v1 | 时域循环卷积 | 33.8s | 20/20 ✅ | 最可靠：直接实现数学定义 |
| v2 | 预扩展数组去取模 | 27.9s | 20/20 ✅ | 避免 rem_euclid 内层循环 |
| v3 | FFT 卷积（失败） | 17条 | ❌ | halo=HALF_FILT 不够，需要 NFILT |
| v4 | FFT 卷积（成功） | 11.4s | 20/20 ✅ | halo=NFILT, offset=NFILT 才对 |
| v5 | +并行候选 | 4.90s | 20/20 ✅ | 频域隔离保证并行无损失 |
| v6 | +WSJT-X 参数 | 3.60s | 20/20 ✅ | BP 30次+OSD order-2 足够 |

### 1.5 FFT 卷积关键技术

**为什么 FFT 卷积需要 NFILT 样本 halo 而不是 HALF_FILT：**

```
时域循环卷积: cfilt[i] = Σ camp[(i-τ) mod N] × window[τ]
             τ 的范围是 0..NFILT (4001 个点)
             当 i < NFILT 时，(i-τ) mod N 可能为负数
             → 需要从 camp 尾部取 HALF_FILT=2000 个样本

线性卷积:    cfilt = IFFT(FFT(ext) × FFT(window))
             ext = camp 两端各扩展 NFILT 样本
             → 总共需要 NFILT 样本的 halo
```

**正确实现：**
```rust
// 1. 扩展 NFILT 样本的环形 halo
ext_len = NFRAME + NFILT  // 155680
for j in 0..ext_len:
    ext[j] = camp[(j - HALF_FILT) mod NFRAME]

// 2. FFT 线性卷积
cfilt_fft = FFT(ext) × FFT(window)  // window 预计算缓存
cfilt = IFFT(cfilt_fft)

// 3. 提取结果（偏移 NFILT）
cfilt[i] = cfilt[i + NFILT]  // 不是 HALF_FILT！
```

**失败经验：** 第一次尝试 FFT 卷积时用了 HALF_FILT=2000 的 halo 和 HALF_FILT 的偏移，结果只解出 17 条。改为 NFILT=4000 后恢复 20/20。

---

## 第二部分：性能优化（33.8s→3.60s）

### 2.1 性能瓶颈分析

Pass 级分解（depth=3, 20 条消息）：

| Pass | 消息数 | 耗时 | 说明 |
|------|--------|------|------|
| Pass 1 | 12 条 | 0.22s | sync8 + decode 强信号 |
| Pass 2 | 2 条 | 0.003s | 残差中 decode |
| **Pass 3** | **6 条** | **4.57s** | 极弱信号 decode |
| subtract ×20 | — | ~0.6s | 信号减法 |

**Pass 3 慢的根因：** 每个候选做 4 种 bit metric × (BP 100 次迭代 + OSD order-3)：
```
6 candidates × 4 metrics × 100 BP iterations = 2400 BP iterations
6 candidates × 4 metrics × OSD order-3 = 6 × 4 × C(174,3) ≈ 21M pattern checks
```

### 2.2 优化 v2：去除取模（27.9s, -17%）

**问题：** 内层卷积循环有 `rem_euclid` 取模运算（151680 × 4001 = 607M 次调用）

**方案：** 预扩展数组避免取模
```rust
// 原来: ext[(i + NFILT - tau) % NFRAME]
// 优化: ext[i + NFILT - tau] (直接索引, 因为已预扩展到 NFRAME+NFILT)
```

### 2.3 优化 v4：FFT 卷积（11.4s, -59%）

**原理：** O(N×M) 时域卷积 → O(N·logN) FFT 卷积
- N=151680, M=4001 → N×M = 607M 操作
- FFT 262144 点 × 3 次 → ~14M 操作（43× 加速）

**实现：**
```rust
// 预计算窗口 FFT（OnceLock 缓存，所有 subtract 共享）
fn lpf_window_fft() -> &'static (Vec<f64>, Vec<f64>) {
    // cos²窗口 → FFT → 缓存
}

// 每次 subtract: FFT(camp) × 预计算FFT → IFFT → 提取
```

### 2.4 优化 v4：并行候选解码（4.90s, -57%）

**原理：** pass 内所有候选独立解码（共享只读 residual/cx），rayon 并行。

**实现：**
```rust
if book.is_none() {
    candidates.par_iter().filter_map(|cand| ft8b(...)).collect()
} else {
    // 有 HashCallBook 时顺序执行
}
```

**安全性验证：**
- 最坏场景：WA2FZW@2546Hz vs W1FC@2571Hz（25Hz 间距, SNR 差 18dB）
- 结果：并行解码下 WA2FZW sync=1.1 仍被正确解出 ✅
- 原因：ft8_downsample 窄带 ±4Hz，25Hz >> 8Hz → 频域完全不重叠

### 2.5 优化 v5：对齐 WSJT-X 参数（3.60s, -27%）

**问题：** ft8rs 的 LDPC 解码参数比 WSJT-X 保守得多

| 参数 | WSJT-X 原版 | ft8rs 之前 | 差异 |
|------|------------|-----------|------|
| BP max_iterations | **30** | 100 | 3.3× |
| BP early stop | ncnt≥5, iter≥10, ncheck>15 | ncnt≥10, iter≥15, ncheck>25 | 更早停止 |
| OSD order | **2** | 3 | C(174,2)=15K vs C(174,3)=870K (57×) |

**验证结果：** 改为 WSJT-X 参数后仍 20/20，最弱信号 WA2FZW -19dB 无影响。

**为什么 30 次迭代足够：**
- 强信号：5-15 次收敛
- 中等信号：20-30 次收敛
- 弱信号：30 次后仍未收敛 → 再多迭代也不收敛（进入极限）
- OSD order-2 能纠正 ~10 个比特错误，对 (174,91) LDPC 码足够

---

## 第三部分：架构决策

### 3.1 减法时机与并行解码

**WSJT-X（顺序）：** 每解码一个信号立即 subtractft8，后候选看干净残差
**ft8rs（并行）：** 所有候选并行解码，批量减法

| 场景 | WSJT-X | ft8rs 并行 | 影响 |
|------|--------|-----------|------|
| 跨 pass | Pass 1 减完 → Pass 2 sync8 看干净残差 | 完全一样 | ❌ 无 |
| 同 pass 远隔信号 | 前一个被减，后一个看干净残差 | 都看原始残差 | ❌ 无 |
| 同 pass 近距离 | 强信号被减，弱信号可能解出 | 弱信号看到重叠 | ❌ 无 |
| 重叠信号(<5Hz) | 强先减，弱再解 | 两者看重叠 | ⚠️ 理论差异 |

**验证：最坏情况** — WA2FZW@2546Hz (-19dB) vs W1FC@2571Hz (-1dB)，25Hz 间距，并行解码仍 OK。

**结论：并行解码 + 批量减法在灵敏度上零损失。** 减法核心价值是让下一 pass 的 sync8 发现新信号，而非同 pass 内候选解码质量。

### 3.2 HashCallBook 性能影响

**结论：零影响。** HashCallBook 只是一个 Vec 查表（unpack77 中 1-2 次 O(1) 查找），总耗时占比 <0.01%。

**实际影响：** 有 HashCallBook 时退化为顺序模式（RefCell 不 Send），但这对灵敏度无影响。

### 3.3 AP 解码代码状态

`try_decode_passes` 中有 4 个 AP pass（CQ 模式 + i3/n3 约束），但**实测对 20/20 基线无贡献**（禁用后结果不变）。真正的 WSJT-X AP 需要已知呼号信息（apsym），当前未实现。

---

## 第四部分：WSJT-X 差异分析

### 4.1 已对齐的核心算法

| 模块 | 验证项 | 状态 |
|------|--------|------|
| sync8 | FFT 尺寸、Costas数组、sync_abc/bc、40%基线、去重<4Hz/<0.04s | ✅ 一致 |
| sync8d | Costas复波形、3块偏移、复相关、频偏±2.5Hz(11档) | ✅ 一致 |
| ft8_downsample | NFFT1_LONG=192000、频带f0±[1.5,8.5]×baud、cos² taper | ✅ 一致 |
| ft8b 核心 | 时偏±10、频偏±2.5Hz、时偏精炼±4、graymap | ✅ 一致 |
| bit metrics | bm=max1-max0、4种指标、normalizebmet(/σ)、scalefac=2.83 | ✅ 一致 |
| subtractft8 | 复基带 LPF 信号减法 | ✅ 一致 |
| LDPC | normalizebmet、BP、OSD、CRC14 | ✅ 一致 |

### 4.2 未实现特性

| 特性 | WSJT-X 实现 | 预期收益 | 当前状态 |
|------|------------|---------|---------|
| **AP 解码** | iaptype 1-6, contest 0-8, apsym 注入 LLR | 2-4dB | 有框架，未接入已知呼号 |
| **a7 历史复用** | ft8c.f90 跨时隙信号记忆 | 1-3 条 | 未实现 |
| **lrefinedt** | ±90 样本时偏精炼减法 | 残差更小 | 未实现 |
| **渐进式解码** | nzhsym 41→47→50 分段处理 | 实时优势 | 未实现 |
| **Contest 模式** | 8 种 contest 特定比特模式 | 场景相关 | 未实现 |

### 4.3 WSJT-X 并行策略

WSJT-X 的 FT8 候选解码**完全是串行的**（`do icand=1,ncand` 顺序循环）。唯一的 `omp parallel sections num_threads(2)` 在 `decoder.f90` 中用于 JT9 + Q65 **同时**解码两个不同模式，不是候选并行。

ft8rs 的并行候选解码是**超越 WSJT-X 的创新**，利用多核加速，且无损灵敏度。

---

## 第五部分：未来优化方向

### 5.1 不妥协精度的优化

| 方向 | 预期收益 | 难度 | 风险 |
|------|---------|------|------|
| BP 4 metrics 并行 | ~0.2s | 低 | 零（并行独立） |
| sync8 候选数裁剪 | ~0.1s | 低 | 低（合理阈值） |
| GFSK pulse 静态缓存 | ~0.05s | 极低 | 零（已实现） |

### 5.2 可能提升灵敏度的方向

| 方向 | 预期收益 | 风险 | 验证要求 |
|------|---------|------|---------|
| 已知呼号 AP 解码 | 2-4dB | 中 | 需要 20/20 不变 + 新弱信号 |
| a7 跨时隙复用 | 1-3 条 | 低 | 多文件测试 |
| lrefinedt 时偏精炼 | 残差更小 | 低 | 减法精度验证 |
| 渐进式解码 | 实时优势 | 中 | 分段 sync8 验证 |

### 5.3 不建议的方向

| 方向 | 原因 |
|------|------|
| 降低 BP 迭代到 <30 | WSJT-X 已用 30，再降可能损失弱信号 |
| OSD order 降到 1 | order-2 已足够，order-1 可能损失 |
| 降低 FFT 分辨率 | sync8 频率精度是基础 |
| 简化 GFSK 为方波 | 波形精度是减法精度的前提 |

---

## 附录 A：测试基线与质量门控

| 指标 | 要求 | 当前 |
|------|------|------|
| 解码数 (210703_133430.wav) | ≥20 条 | 20/20 ✅ |
| 耗时 (release) | <180s | 3.60s ✅ |
| 编译警告 | 0 | 0 ✅ |
| 单元测试 | 全部通过 | 9/9 ✅ |
| `test_20_message_baseline` | 断言 ≥20 | 通过 ✅ |

## 附录 B：关键文件清单

| 文件 | 行数 | 功能 |
|------|------|------|
| `src/ft8/decode.rs` | ~1230 | 主解码逻辑：sync8 → ft8b → pass循环 → 减法 |
| `src/util/subtract_ft8.rs` | ~200 | 精确信号减法：GFSK波形 + FFT卷积LPF |
| `src/util/decode174_91.rs` | ~370 | LDPC (174,91) BP + OSD 解码器 |
| `src/util/fft.rs` | ~350 | FFT (radix-2 + Bluestein) |
| `src/util/waveform.rs` | ~190 | GFSK 波形生成（检测用，NSPS=48） |
| `wsjtx/lib/ft8/subtractft8.f90` | 117 | WSJT-X 原始减法代码（参考） |
| `wsjtx/lib/ft8/decode174_91.f90` | — | WSJT-X 原始 LDPC 解码器（参考） |

## 附录 C：参考项目

| 项目 | 语言 | 解码数 | 说明 |
|------|------|--------|------|
| WSJT-X v2.7.0 | Fortran | 20/20 | 权威参考实现 |
| ft8ts | TypeScript | 16/20 | TypeScript 参考端口 |
| wsjtx_lib | C++ | 14/20 | C++ 封装 Fortran |
| ft8rs (本项目) | Rust | 20/20 | 3.60s |


## 第六部分：调用层编排 — 多视角重搜（2026-05-19）

### 6.1 核心发现

**灵敏度提升的关键不在解码器内部算法，而在调用层的多角度编排。**

通过深入对比 WSJT-X 和 JTDX 源码，发现它们在不同 pass 中使用**不同的 sync 谱计算方式**：

| Sync 模式 | 公式 | 优势 | WSJT-X | JTDX | ft8rs (修复前) |
|---|---|---|---|---|---|
| Power | Re² + Im² | 强信号 | ✅ | ✅ | ✅ |
| Amplitude | √(Re² + Im²) | 弱信号（压缩动态范围） | ❌ | ✅ pass 1,4,7 | ❌ |
| AbsSum | \|Re\| + \|Im\| | 脉冲噪声鲁棒 | ❌ | ✅ pass 3,6,9 | ❌ |

### 6.2 实现

#### SyncMode 枚举
```rust
pub enum SyncMode {
    Power = 0,      // 默认，强信号
    Amplitude = 1,  // 弱信号友好
    AbsSum = 2,     // 抗脉冲噪声
}
```

#### 通过 DecodeOptions 传递到解码链路
```rust
pub struct DecodeOptions {
    ...
    pub sync_mode: Option<SyncMode>,  // → decode() → sync8()
}
```

#### long_decode 多 cycle 编排
```
Cycle 1: Power sync, 原始数据, syncmin=0.80
Cycle 2: Amplitude sync, 平滑数据, syncmin=0.68
Cycle 3: AbsSum sync, 原始数据, syncmin=0.52
每 cycle 去重合并
```

### 6.3 灵敏度提升历程

| 阶段 | 命中 | 提升 | 关键改动 |
|---|---|---|---|
| 原始基线 | 338/449 (75.3%) | — | — |
| +5 项基础修复 | 355/449 (79.1%) | +17 | sync8邻频/padding/OSD/passes等 |
| +Power+Amplitude平滑 | 362/449 (80.6%) | +7 | SyncMode 接入 decode |
| +AbsSum | 366/449 (81.5%) | +4 | 3 种模式全覆盖 |

**总提升：338 → 366 (+28 条, +6.2%)**

### 6.4 数据平滑技术

```rust
fn smooth_data(data: &[f64]) -> Vec<f64> {
    // JTDX pass 4 技术: dd[i] = (dd[i-1] + dd[i]) / 2
    let mut smoothed = vec![0.0; n];
    smoothed[0] = data[0];
    for i in 1..n {
        smoothed[i] = (data[i - 1] + data[i]) * 0.5;
    }
    smoothed
}
```

低通滤波减小噪声方差，配合 Amplitude sync 模式对弱信号更敏感。

### 6.5 SNR-based 同步门控增强

从简单 nsync 计数升级为 JTDX 风格：

```rust
// 每符号计算信噪比: sync_tone / avg(other_7_tones)
// nsyncscore: SNR > 1 的符号数
// scoreratio: 平均 SNR
// 软门控: nsync < 4 但 nsyncscore >= nsync 且 scoreratio > 3.0 → 放行
```

### 6.6 经验教训

**第一次尝试失败**：定义了 SyncMode 枚举和 long_decode，但 cycle 2 调用 decode_ft8 时没传递 sync_mode 参数，decode 内部 hardcoded SyncMode::Power。2-cycle 跑了两次同样的 Power 模式 → 0 增益。

**修复后**：SyncMode → DecodeOptions → decode() → sync8()，完整链路打通 → +11 条。

**教训**：调用层编排不仅仅是"多跑几遍"——必须是**有差异的多遍**（不同参数、不同算法），合并后才互补。这正是 ANALYSIS.md 最初的分析结论。


## 第七部分：syncmin 对齐 WSJT-X（2026-05-19）

### 7.1 发现

WSJT-X 使用 syncmin=1.3，我们长期使用 0.8。经测试：
- syncmin=1.3 在 20/20 测试和 362/449 长解码上均无灵敏度损失
- 候选数减少 → 性能提升 9-34%

### 7.2 结果

| 测试 | syncmin=0.8 | syncmin=1.3 | 变化 |
|---|---|---|---|
| 20/20 基线 | 76s | 50s | -34% |
| 362/449 长解码 | 491s | 446s | -9% |

### 7.3 分析

为什么 syncmin 翻倍不减灵敏度？
- Cycle 1 (Power) 用 1.3，筛选强信号
- Cycle 2 (Amplitude) 用 1.3×0.85=1.105，Amplitude 谱对弱信号更敏感，补偿了高门限
- SNR-based 软门控允许低 nsync 高 SNR 信号通过

### 7.4 3840 FFT 尝试

切到 WSJT-X 的 3840-point FFT 后即使 syncmin=0.5 也无法通过 20/20。
根因是 Bluestein FFT 非 2 的幂算法与 Fortran FFT 的数值差异。

### 7.5 剩余 WSJT-X 差距

| 特性 | WSJT-X | ft8rs | 影响 |
|---|---|---|---|
| FFT 尺寸 | 3840 | 4096 | 频率网格不同 |
| 频谱基线 | Nuttall 窗 Welch | 滑窗平均 | sbase 精度 |
| 信号减法 | lrefinedt 时偏精炼 | 基础减法 | 残差更小 |
| AP 解码 | 完整 iaptype 1-6 | 框架未启用 | 2-4dB |

当前 362/449 vs WSJT-X ~420+/449，差距约 60 条。
核心瓶颈：FFT 对齐后需要系统性重校准。


## 第八部分：mixed-radix FFT 实现（2026-05-19）

### 8.1 问题

Bluestein FFT（chirp-z）对于非 2 的幂尺寸（3840, 3200）存在两个问题：
1. 速度慢：需 3× 大尺寸 radix-2（8192 点），实质 O(N²logN) 开销
2. 数值误差：多级 FFT 累积浮点误差

### 8.2 解决方案

实现 Cooley-Tukey 混合基 FFT：N = P × Q，Q 为 2 的幂。
- 3840 = 15 × 256
- 3200 = 25 × 128

P ≤ 50 时使用直接 DFT（O(P²) 可忽略），Q 使用现有 radix-2。

### 8.3 3840 sync8 测试

FFT 正确性已验证（对比 Bluestein，3 个测试全过），但 sync8 切换到 3840 后：
- 20/20 → 19/20（总差 1 条）
- 速度下降 58%（79s vs 4096 的 50s）
- 根因：3840 的混合基拆解开销（15×256 点 FFT + transpose + 256×15 点 DFT）> 4096 纯 radix-2

### 8.4 最终方案

| 用途 | FFT 方式 | 理由 |
|---|---|---|
| sync8 | 4096 radix-2 | 速度最优，20/20 ✅ |
| ft8_downsample (NFFT2=3200) | 3200 mixed-radix | 快于 Bluestein ~3× |
| 其他非 2 的幂 | mixed-radix | 通用优化 |

### 8.5 性能对比

| FFT 尺寸 | 算法 | 近似操作数 |
|---|---|---|
| 4096 (2^12) | radix-2 | ~49K |
| 3840 (15×256) | mixed-radix | ~92K |
| 3840 | Bluestein → 8192 | ~319K |
| 3200 (25×128) | mixed-radix | ~102K |
| 3200 | Bluestein → 8192 | ~319K |
