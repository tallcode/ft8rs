# FT8 架构级灵敏度提升计划

> 当前：362/449 (80.6%)，446s，20/20 ✅  
> 目标：400+/449，<500s  
> 参考：WSJT-X ~420+/449

---

## Phase 1: sbase 基线重写（预计 +10-20 条）

### 问题
当前 `compute_baseline` 只是滑窗平均 sync8 的 372 个短 FFT 谱。
WSJT-X 的 `get_spectrum_baseline` 使用 Nuttall 窗 + Welch 法（93 个 3840 点重叠 FFT）。

### 方案
```
1.1 实现 nuttal_window(NFFT1=3840) 
    → 参考 WSJT-X nuttal_window.f90，生成 Nuttall 窗系数

1.2 重写 compute_baseline → get_spectrum_baseline
    → 对 dd 做 93 个重叠 FFT（步长 NFFT1/2=1920）
    → 每个 FFT 前乘 Nuttall 窗
    → savg = 所有 FFT 的 abs² 求和平均
    → baseline(savg, nfa, nfb) 滑动中值/均值

1.3 用 sbase 做 sync 归一化
    → sync8 的 sync 值除以对应频点的 sbase
    → 现在 sync 归一化用的是 40% 分位全局值
    → 改为频点级的 sbase 归一化（更精细的频域均衡）

1.4 启用 sbase LLR 归一化
    → xbase = 10^(0.1*(sbase[freq_bin] - 40))
    → 测试不同 offset 值（WSJT-X 用 40，我们可能需要校准）
    → 对 bmetrics 乘以 1/sqrt(xbase)
```

### 验证
- 20/20 不能退步
- 362+ 必须保持
- sbase 归一化后的 sync 值更稳定（减少频域偏差）

---

## Phase 2: AP 解码接入（预计 +5-15 条）

### 问题
AP 解码框架已存在（Pass 9/10），但从未被测试使用。
没有传入 mycall/hiscall → AP passes 跳过 → 零贡献。

### 方案
```
2.1 从解码结果自动提取 mycall/hiscall
    → 扫描已解码消息，提取 CQ 呼号作为 hiscall
    → "CQ XX1XXX" → hiscall = "XX1XXX"
    → 在下一段使用这些呼号做 AP

2.2 接入 AP Pass 9/10
    → 传入 mycall + hiscall 到 decode_ft8
    → Pass 9: MYCALL ??? ???（iaptype=2）
    → Pass 10: MYCALL HISCALL ???（iaptype=3）
    → 在 long_decode 中每段尝试多个 hiscall 候选

2.3 长解码集成
    → 段 N 解码 → 提取 hiscall 候选列表
    → 段 N+1 解码时传入这些 hiscall 做 AP
    → AP 命中可在更低 SNR 解码（2-4 dB 增益）
```

### 验证
- 20/20 不能退步
- 362+ 保持，期望 +5-15 条
- AP 命中消息标注（区分常规 vs AP 解码）

---

## Phase 3: 跨段信号持久化（预计 +5-10 条）

### 问题
目前 signal_memory 只保存 freq/dt/msg，未用于实际检测。
JTDX 的 evencq/oddcq 保存完整复符号，实现匹配滤波关联。

### 方案
```
3.1 保存复符号 cs(0:7,79) 到 signal_memory
    → 修改 Ft8bResult 增加 cs 字段
    → decode_ft8 返回 cs 数据

3.2 跨段匹配滤波
    → 对 signal_memory 中的信号，在下一段做复相关
    → 如果相关峰值超过阈值，直接输出已保存消息
    → 跳过完整 LDPC 解码（更快 + 更低 SNR 检测）

3.3 简化版（优先）
    → 不保存完整复符号，只用 freq+dt 匹配
    → 如果候选在已保存信号 ±3Hz, ±0.5s 内，且 sync>0.5
    → 用已保存消息做 AP hint 尝试 LDPC
```

### 验证
- 20/20 不能退步
- 段间关联命中率提升
- 计算开销可控（窄带搜索）

---

## Phase 4: 参数精调（预计 +5-10 条）

### 方案
```
4.1 per-pass syncmin 差异化
    → Pass 0: syncmin × 1.0（标准）
    → Pass 1: syncmin × 0.9（放宽）
    → Pass 2: syncmin × 1.0
    → Pass 3: syncmin × 1.2（最严，减少误检）

4.2 max_passes 动态调整
    → 如果前一轮无新解码，不跑更多 pass
    → WSJT-X: pass 2 only runs if pass 1 decoded new
    → 减少无用计算

4.3 nagain 参数优化
    → 减小 nagain 频带窗口（±20→±15Hz）
    → 降低 nagain syncmin（×1.2 而非 ×1.5）
    → nagain 用 Amplitude 模式（而非 Power）
```

---

## 进度与优先级

| Phase | 内容 | 预计收益 | 难度 | 风险 |
|---|---|---|---|---|
| 1 | sbase Nuttall 窗重写 | +10-20 | 中 | 20/20 可能退步 |
| 2 | AP 解码接入 | +5-15 | 低 | 已有框架 |
| 3 | 跨段信号关联 | +5-10 | 中 | 需要暴露 cs 数据 |
| 4 | 参数精调 | +5-10 | 低 | 纯调参 |

**建议推进顺序：Phase 2 → Phase 1 → Phase 4 → Phase 3**

Phase 2（AP 解码）风险最低——框架已有，只需传入参数。
Phase 1（sbase）收益最大但需要校准 Nuttall 窗。
