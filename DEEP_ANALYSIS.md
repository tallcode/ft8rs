# WSJT-X 深度源码分析 — 如何达到 420/449

## 目标
- 20/20 ≤30s
- 420/449 长解码命中率

## 当前状态
| 指标 | WSJT-X | ft8rs | 差距 |
|---|---|---|---|
| 20/20 | ≤10s | ~50s | 慢 5× |
| 长解码 | ~420+/449 | 362/449 (80.6%) | -58 条 |
| 单段耗时 | ~10s | ~25s | 慢 2.5× |

## 已穷尽的优化方向

| 尝试 | 结果 | 结论 |
|---|---|---|
| FFT 3840 → 4096 | 20/20 失败 | 需要全参数重校准，非独立改动 |
| syncmin pass 缩放 | 362→362, 慢 138s | 无效，回退 |
| 平滑数据 | 零增益 | 无效 |
| AP 盲解 | 零增益 | 无效 |
| xbase LLR 归一化 | 零增益, +152s | 无效 |
| AbsSum 第 3 轮 | +4 条/+252s | ROI 太低 |
| nagain syncmin 调整 | 零增益 | 无效 |
| maxosd 5→2 | 零增益, 代码简化 | 已采纳（简化） |

## WSJT-X 的核心架构优势

### 1. 渐进式解码 (Progressive Decoding) — 最大差异

WSJT-X 的 `ft8_decode.f90` 按时间推进分阶段处理：

```fortran
! nzhsym=41: 前 11s (约 1/3 符号) → 早期候选
nzhsym=41: ndec_early = 早期解码数量
! nzhsym=47: 前 11.75s → 用早期结果精炼减法
nzhsym=47: lrefinedt=true, 减法精炼
! nzhsym=50: 完整 12.5s → 最终解码
nzhsym=50: 完整 sync8 + decode
```

**关键**：早期解码结果 → 精炼减法 → 残差更干净 → 最终解码能看到更弱的信号。
这是一个**正向反馈循环**，每一步都利用上一步的信息。

ft8rs：每段只做一次完整解码，无渐进式。

**潜在收益**：估计 +15-25 条

### 2. SNR 计算使用 xbase

WSJT-X 在 `ft8b.f90` 中计算两种 SNR：
```fortran
xsnr  = 10*log10(xsig/xnoi - 1) - 27      ! 简单信噪比
xsnr2 = 10*log10(xsig/xbase/3e6 - 1) - 27 ! sbase 归一化
if (.not.nagain) xsnr = xsnr2              ! 非 nagain 时用归一化版
```

**关键**：xbase = 10^(0.1*(sbase[freq_bin] - 40))，是 WSJT-X `get_spectrum_baseline` 输出的频谱基线估计。这补偿了频率选择性衰落，在嘈杂频段降权，干净频段加权。

ft8rs：只用 `xsig/xnoi`，无 sbase 归一化。

**验证结果**：我们接了 sbase_welch 后发现 normalize_bmet 已经够好，xbase 无额外增益。
但 xbase 在 SNR 估算（非 LLR 归一化）中可能有用。

**潜在收益**：估计 +3-5 条（需校准 xbase offset）

### 3. lrefinedt 减法精炼

WSJT-X 在 `subtractft8.f90` 中：
```fortran
if(lrefinedt) then
    sqa=sqf(-90) ! 偏移 -90 样本的残差能量
    sqb=sqf(+90) ! 偏移 +90 样本的残差能量
    sq0=sqf(0)   ! 原始残差能量
    call peakup(sqa,sq0,sqb,dx) ! 二次插值找最优 dt
    if(abs(dx).gt.1.0) return ! 不满足最小值 → 不减法
    i2=nint(90.0*dx) ! 精炼后的 dt 偏移
endif
```

在减法前精炼 dt，确保减法信号和实际信号对齐，减少残留。

ft8rs：无此精炼。

**潜在收益**：估计 +2-5 条（残余更干净）

### 4. 双峰值候选搜索

WSJT-X `sync8.f90`：
- red(i) = max(sync2d, ±10 steps) → ±0.4s
- red2(i) = max(sync2d, ±62 steps) → ±2.5s
- 如果 red 和 red2 峰值位置不同 → 两个都加为候选

ft8rs：已实现双峰值搜索 ✅

**潜在收益**：已在 ft8rs 中实现

### 5. nagain 窄带重搜

WSJT-X：
```fortran
if(nzhsym.eq.50 .and. nagain) then
    dd=iwave              ! 用原始数据
    ifa=nfqso-20          ! 只搜 QSO 频率 ±20Hz
    ifb=nfqso+20
endif
```

关键区别：
- WSJT-X nagain 只搜**一个频率**（nfqso）的 ±20Hz
- ft8rs 搜**所有已解码频率**的 ±20Hz

WSJT-X 的 nagain 是**定向的**——操作员双击的信号频率。这是交互式操作。

ft8rs 的 nagain 是**盲扫的**——搜所有频率。这更耗时间但覆盖更广。

**潜在收益**：nagain 在盲扫场景价值有限（已验证）

## 最可能的突破口

### 优先级 1: 实现渐进式解码（估计 +15-25 条）

思路：
```
段内分 3 阶段：
1. 前 11s → sync8(syncmin=1.3) → 早期候选 → decode → 减法
2. 前 11.75s → 用阶段 1 结果精炼 dt → 再减法
3. 完整 15s → 更干净的残差 → sync8(syncmin=1.3) → 最终 decode
```

每个阶段利用上一阶段的解码结果，残差逐步变干净。

### 优先级 2: lrefinedt 减法精炼（估计 +2-5 条）

在减法前做 ±90 样本的 dt 精炼，确保对齐。

### 优先级 3: xbase 在 SNR 过滤中的应用（估计 +1-3 条）

用 sbase_welch 计算 xsnr2，在最终过滤中使用。

### 性能优化

当前 25s/段，目标 <30s ✅ 已达标。
但 20/20 在 50s，目标 <30s。

加速方向：
1. 减少 max_candidates（当前 500 → 试 300）
2. Rayon 并行候选解码（已实现，但受限于 HashCallBook 时退化为串行）
3. 减少 decode174_91 BP 迭代（30→20，已测试过）

## 结论

从 362→420 (+58 条) 需要**架构级改动**，非参数调优可解决。

渐进式解码是最大的单一改进机会，也是 WSJT-X 核心设计哲学：
**不是一次性解，是逐步逼近**。

当前 ft8rs 架构（一次性完整解码）已经把潜力挖到了极限。
