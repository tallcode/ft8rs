# FT8 解码技术报告

> **ft8rs** — 纯 Rust FT8/FT4 编码器与解码器
>
> 基准测试文件：`tests/ft8/210703_133430.wav`（12kHz, 15s 短解码）
> 长解码测试：`tests/ft8/230208_140300.wav`（48kHz, ~4min, 18 segments）
>
> 参考实现：[WSJT-X v2.7.0](https://wsjt.sourceforge.io/wsjtx.html) (Fortran), [ft8ts](https://github.com/e04/ft8ts) (TypeScript)

---

## 里程碑总览

### 灵敏度提升

| 阶段 | 短解码 | 长解码 | 关键突破 |
|------|--------|--------|---------|
| 初始 | 16/20 | 338/449 (75.3%) | 无信号减法 |
| +subtract_ft8 | **20/20** | — | 精确信号减法消除强信号掩蔽 (3个bug修复) |
| +SyncMode多模式 | 20/20 | 366/449 (81.5%) | Power/Amplitude/AbsSum 三模式循环 |
| +syncmin=1.3 | 20/20 | 366/449 (81.5%) | 对齐WSJT-X，候选数减少34% |

### 性能优化历程（短解码）

| 版本 | 耗时 | 加速 | 技术 |
|------|------|------|------|
| v0 基线 | 7.0s | 1× | 无信号减法 |
| v1 subtract_ft8 | 33.8s | — | 时域 O(N×M) 循环卷积 |
| v4 FFT卷积+并行 | 4.90s | 6.9× | O(N·logN) 卷积 + rayon 并行候选 |
| v5 WSJT-X参数 | 3.60s | 9.4× | BP 30次 + OSD order-2 |
| **v6 rustfft** | **~2.0s** | **~13×** | 替换手写FFT为 rustfft (混合基→库) |
| **v7 平行段** | **~1.9s** | **~15×** | extract_soft_symbols零拷贝 + shift优化 |

### 性能优化历程（长解码）

| 版本 | 耗时 | 加速 | 技术 |
|------|------|------|------|
| 基线 | ~316s | 1× | 顺序处理18段 |
| +rustfft | ~174s | 1.8× | FFT加速 |
| **v7 平行段** | **~23s** | **~13.7×** | 🚀 rayon 并行段处理 |

### 最终指标

| 指标 | 要求 | 当前 | 状态 |
|------|------|------|------|
| 短解码 (20-消息文件) | ≤2s | **~1.9s** | ✅ |
| 长解码 (18段文件) | ≤100s | **~23s** | ✅ |
| 短解码灵敏度 | 20/20 | **20/20** | ✅ |
| 长解码灵敏度 | ~75%+ | **366/449 (81.5%)** | ✅ |
| 编译警告 | 0 | **0** | ✅ |

---

## 第一部分：核心算法实现

### 1.1 解码流程

```
sync8 (2D同步搜索) → 候选列表 → ft8b (单候选解码):
  load_coarse_downsample → find_best_time_offset → find_best_frequency_shift
  → refine_time_offset → extract_soft_symbols → build_bit_metrics
  → try_decode_passes (BP+OSD+AP)
→ 批量 subtract_ft8 → 下一pass
```

### 1.2 已对齐 WSJT-X 的核心模块

| 模块 | 验证项 | 状态 |
|------|--------|------|
| sync8 | FFT 4096, Costas, sync_abc/bc, 40%基线, 去重 | ✅ |
| sync8d | Costas复波形, 3块偏移, 复相关 | ✅ |
| ft8_downsample | NFFT1_LONG=192000, cos² taper | ✅ |
| ft8b 核心 | 时偏±10, 频偏±2.5Hz, 时偏精炼±4 | ✅ |
| bit metrics | 4种指标, normalize_bmet, scalefac=2.83 | ✅ |
| subtract_ft8 | GFSK波形 + FFT卷积LPF | ✅ |
| LDPC | BP+OSD+CRC14, WSJT-X参数(30次,order-2) | ✅ |
| SyncMode | Power/Amplitude/AbsSum 枚举 | ✅ |

---

## 第二部分：关键技术决策

### 2.1 信号减法 (subtract_ft8)

FT8减法从 16→20 条的最关键突破。修复了 3 个隐藏 bug：

| Bug | WSJT-X 正确实现 | ft8rs 原始错误 | 后果 |
|-----|----------------|--------------|------|
| FFT 尺寸 | NFFT=NMAX=180,000 | 零填充到 262,144 | LPF窗口位置偏移 |
| IFFT 归一化 | 正向/反向都不归一化 | 反向 IFFT 归一化 1/N | LPF增益错误 N 倍 |
| cshift 方向 | cshift(正)=向左 | rotate_right(向右) | 窗口再次偏移 |

**FFT 卷积关键技术**：需要 NFILT=4000 样本 halo（不是 HALF_FILT=2000），否则线性卷积截断丢失尾部能量。

### 2.2 多模式 Sync 循环

JTDX 启发：不同 pass 使用不同 sync 谱计算方式：

| Sync 模式 | 公式 | 优势 | 长解码额外命中 |
|---|---|---|---|
| Power | Re² + Im² | 强信号 | 基准 |
| Amplitude | √(Re² + Im²) | 弱信号(压缩动态范围) | +7 条 |
| AbsSum | │Re│ + │Im│ | 脉冲噪声鲁棒 | +4 条 |

**关键教训**：第一次实现时 cycle 2 没传递 sync_mode 参数，两次都是 Power → 零增益。修改后 +11 条。

### 2.3 syncmin 对齐

WSJT-X 使用 syncmin=1.3（不是 0.8）：

| 测试 | syncmin=0.8 | syncmin=1.3 | 变化 |
|---|---|---|---|
| 20/20 基线 | 76s | 50s | -34% |
| 候选数 | ~450 | ~290 | -35% |

Amplitude 模式对弱信号更敏感，补偿了高门限；SNR-based 软门控( jtDX 风格 )允许低 nsync 高 SNR 信号通过。

### 2.4 并行策略

**WSJT-X 完全串行**（唯一 `omp parallel sections` 用于 JT9+Q65 多模式同时解码）

**ft8rs 两层并行（超越 WSJT-X）：**
1. **候选并行**：pass 内所有候选独立解码 (rayon) — 无损灵敏度
2. **段并行**：long_decode 中所有 15 秒段独立处理 (rayon) — **6× 加速**

安全性验证：WA2FZW@2546Hz(-19dB) vs W1FC@2571Hz(-1dB)，25Hz 间距，并行仍 OK。
原因：ft8_downsample 窄带 ±4Hz，25Hz >> 8Hz → 频域完全不重叠。

### 2.5 FFT 引擎演进

| 阶段 | 算法 | 3200点 | 4096点 | 结论 |
|---|---|---|---|---|
| 初始 | 手写 radix-2 + Bluestein | ~319K ops | N/A | Bluestein 非2幂慢3× |
| 中期 | 手写 mixed-radix | ~102K ops | ~92K ops | 通用但代码复杂 |
| **最终** | **rustfft** | ~100K ops | ~49K ops | **最优：库函数+零维护** |

选择 rustfft 理由：
- 针对 x86/ARM 自动 SIMD 优化
- 零手写代码，减少 bug
- 线程局部规划器 + 复用 scratch buffer

### 2.6 subtract_ft8 与并行解码的交互

| 场景 | WSJT-X | ft8rs | 影响 |
|------|--------|-------|------|
| 跨 pass | Pass 1 减完 → Pass 2 看干净残差 | 完全一样 | ❌ 无 |
| 同 pass | 前一个被减，后一个看干净 | 都看原始残差 | ❌ 无 |
| 重叠信号(<5Hz) | 强先减，弱再解 | 两者看叠加 | ⚠️ 理论差异 |

**结论**：减法核心价值是让下一 pass 的 sync8 发现新信号，非 pass 内候选质量。

---

## 第三部分：性能优化技术细节

### 3.1 extract_soft_symbols 零拷贝

重构函数签名从 `(&cd0_re, &cd0_im, ibest, &mut workspace)` 改为 `(ibest, &mut workspace)`，直接访问 workspace.cd0_re/cd0_im。避免 3200×2 f64 的 clone。

### 3.2 ft8_downsample shift 优化

当 shift=0 时跳过 circular copy 循环。常见于精确频率对齐场景。

### 3.3 sync2d buffer 修复

sync2d 原只分配 width × NHSYM = 46500，但 `(ib-ia+1) × width` 可达 123875（当 freq_low=100, freq_high=3000）。改为 `width × half_size` 覆盖全频范围。

### 3.4 long_decode 并行段处理

```rust
// 每个段独立处理，使用独立的 HashCallBook
segment_data.par_iter().map(|(seg, data)| {
    let seg_book = Rc::new(HashCallBook::new());
    progressive_decode(..., &seg_book, ...)
}).collect();
```

段间无依赖 → 完美并行。18 段从 146s 降到 ~23s（6.4×，受线程数/内存带宽限制）。

---

## 第四部分：未实现特性与灵敏度瓶颈

### 4.1 已知 WSJT-X 特性对比

| 特性 | WSJT-X | ft8rs | 预期增益 |
|------|--------|-------|---------|
| **渐进式解码** | ✅ nzhsym 41→47→50 | 部分实现 | +15-25 条 |
| **AP 解码** | ✅ iaptype 1-6 | 框架未启用 | +2-4dB |
| **lrefinedt** | ✅ ±90 样本精炼 | 已实现但未全面使用 | +2-5 条 |
| **a7 跨时隙复用** | ✅ 跨段信号记忆 | 无 | +1-3 条 |
| **Contest 模式** | ✅ 8 种模式 | 无 | 场景相关 |
| **xbase SNR 归一化** | ✅ sbase补偿 | normalize_bmet 替代 | +1-3 条 |

### 4.2 当前灵敏度天花板分析

长解码 366/449 (81.5%) vs WSJT-X ~420+/449 (~93%)，差距 ~54 条。

**最大瓶颈：渐进式解码**

WSJT-X 的 `ft8_decode.f90` 核心设计哲学：
```
nzhsym=41: 前 11s → 早期强信号候选 → subtract
nzhsym=47: 前 11.75s → lrefinedt 精炼减法
nzhsym=50: 完整 15s → 干净残差上的最终解码
```
每步利用上一步结果，残差逐步变干净。这是**正向反馈循环**。

ft8rs long_decode 已实现 progressive_decode（3 stage），但段内仍是一次性完整 15s 解码，缺少 nzhsym 截断逻辑。

### 4.3 代码状态

| 模块 | 文件 | 行数 | 功能 |
|------|------|------|------|
| 主解码 | src/ft8/decode.rs | ~1482 | sync8 → ft8b → pass → subtract |
| 信号减法 | src/util/subtract_ft8.rs | ~350 | GFSK + FFT卷积LPF |
| LDPC | src/util/decode174_91.rs | ~370 | BP + OSD |
| FFT | src/util/fft.rs | ~280 | rustfft 封装 |
| 长解码 | src/util/long_decode.rs | ~405 | 并行段处理 + 渐进式 |

### 4.4 AP 解码框架

`try_decode_passes` 中有 4 个 AP pass（CQ/标准/呼号约束），实测对 20/20 基线无额外贡献。真正 WSJT-X AP 需要已知呼号信息（apsym），当前未接入。

---

## 第五部分：经验教训

1. **"多跑几遍"必须是"有差异的多遍"** — SyncMode 第一次实现没传参，两次跑同一模式 → 零增益
2. **FFT 卷积 halo 必须 = NFILT** — 用 HALF_FILT 只出 17/20，改 NFILT 恢复 20/20
3. **3 个 bug 叠加效果难以定位** — FFT尺寸、IFFT归一化、cshift方向 每个单独 bug 只产生轻微偏差，叠加后才致命
4. **并行候选解码安全** — 频域隔离(±4Hz)保证远距离信号互不干扰
5. **库函数优于手写** — rustfft 替代手写 FFT → 代码减少 350 行 + 速度提升 2-3×

---

## 附录：测试基线

| 测试 | 命令 | 通过标准 |
|------|------|---------|
| 短解码 | `cargo test test_20_message_baseline --release` | ≥20 条, <2s |
| 长解码快速 | `cargo test test_segment_decode_long_quick --release` | ≥57 条 |
| 长解码完整 | `cargo test test_segment_decode_long -- --ignored` | ≥75% match |
| 单元测试 | `cargo test` | 9/9 通过 |
