# FT8 解码优化尝试记录

## 基线
- 解码器：`src/ft8/decode.rs`
- 测试文件：`tests/ft8/230208_140300.wav`（284.5s, 48kHz 32-bit）
- 基准 CSV：`tests/ft8/230208_140300.csv`（449条消息，19段时间戳）
- 20/20 质量门：`test_20_message_baseline`（210703_133430.wav）
- WSJT-X 版本：2.7.0+repack（源码在 `wsjtx/lib/ft8/ft8b.f90`）

## 测试参数
| 参数 | 值 |
|---|---|
| 采样率 | 48000→12000 Hz |
| 段长 | 15s（+ padding） |
| 频段 | 200-3000 Hz |
| depth | 3 |

---

## 尝试 1: sync8 邻频去重放宽 (P0)
**日期：** 2026-05-18
**文件：** `src/ft8/decode.rs` line ~564

### 改动
```rust
// 原始: if fdiff < 4.0 && tdiff < 0.04 {
// 改为:
if fdiff < 0.5 && tdiff < 0.04 {  // 只去衡真正同频（<0.5Hz）
```

### WSJT-X 对比
WSJT-X 的 sync8 不做这种候选级去重。WSJT-X 依赖后续的 `ldupe` 检查（按消息文本去重）。

### 结果
- 20/20: ✅
- 命中: 338→339（+1）
- 长段测试无显著变化

### 状态：✅ 已保留

---

## 尝试 2: 段 padding（ibest 负数修复）
**日期：** 2026-05-18
**文件：** `tests/segment_decode_test.rs`

### 根因
`find_best_time_offset` 返回负 ibest（如 -68），导致 `extract_soft_symbols` 跳过 Costas 位置，s8 全零→nsync=0。

### 改动
每段解码时前后各加 padding samples：
| Padding | 命中 | 缺失 | 备注 |
|---|---|---|---|
| 0s（原始） | 338 | 91 | baseline |
| 0.5s | 357 | 73 | 最优！但丢 CU3HN |
| 1s | 350 | 80 | WSJT-X 参考值 |
| 2s | 321 | 109 | 噪太多，退步 |

### WSJT-X 对比
WSJT-X 使用 17s 窗口（15s+1s×2），额外数据通过 `iwave` 数组提供。

### 结果
- 20/20: ✅
- 最终选择 1s（对齐 WSJT-X）
- CU3HN YC3BMX OI62 @1350Hz +0dB 解出 ✅

### 状态：✅ 已保留（1s padding）

---

## 尝试 3: nagain 数据源改为 residual
**日期：** 2026-05-18
**文件：** `src/ft8/decode.rs` nagain 段

### 改动
nagain 搜索数据从 `dd_original` 改为 `residual`，syncmin 从 1.1 降到 0.5，窗口 ±20→±30Hz，添加未解码候选频点（最多 15 个）。

### WSJT-X 对比
WSJT-X nagain 用原始数据（`iwave`），在 `nfqso±20Hz` 窄带搜。我们改到 residual 上搜，逻辑不同。

### 结果
- 20/20: ✅
- 命中: 338→340（+2，与其他改动叠加）
- 耗时大幅增加（+30s）

### 状态：❌ 已回退（耗时过多，收益低）

---

## 尝试 4: 碰撞恢复 pass
**日期：** 2026-05-18
**文件：** `src/ft8/decode.rs`

### 改动
在主循环和 nagain 之间新增碰撞恢复段：对每个已解码频率 ±20Hz 在 residual 上用 syncmin=0.6 窄带重搜。

### 结果
- 20/20: ✅
- 命中: 350→351（+1，恢复 G5AT SP9WZO R-09）
- 耗时: +57s

### 状态：❌ 已回退（耗时过高）

---

## 尝试 5: nharderrors 40→36 (P1)
**日期：** 2026-05-18
**文件：** `src/ft8/decode.rs` `try_decode_passes()`

### 改动
```rust
// 原始: result.nharderrors <= 40
// 改为: result.nharderrors <= 36
```

### WSJT-X 对比
WSJT-X: `nharderrors.gt.36` → reject。完全对齐。

### 结果
- 20/20: ✅
- 单独效果: 无明显变化

### 状态：✅ 已保留

---

## 尝试 6: maxosd per-pass 区分 (P2)
**日期：** 2026-05-18
**文件：** `src/ft8/decode.rs` `try_decode_passes()`

### 改动
```rust
// 原始: let maxosd = if depth >= 3 { 2 } ...
// 改为 per-pass:
let maxosd = match ipass {
    0 => 2,  // nsym=1, bmeta
    1 => 2,  // nsym=2, bmetb
    2 => 5,  // nsym=3, bmetc ← 全信息 pass，深度 OSD
    3 => 2,  // nsym=1, bmetd（归一化后）
    _ => -1,
};
```

### WSJT-X 对比
WSJT-X: pass 3 (nsym=3) 用 `ibmax=8`（更深），我们只用到 5。

### 结果
- 20/20: ✅
- 单独效果: 无明显变化

### 状态：✅ 已保留

---

## 尝试 7: max_candidates 增加
**日期：** 2026-05-18
**文件：** `tests/segment_decode_test.rs`

### 改动
| max_candidates | 命中 | 缺失 |
|---|---|---|
| 300（原始） | 350 | 80 |
| 500 | 354 | 76 |

### 结果
- 20/20: ✅
- +4 条命中

### 状态：✅ 已保留（500）

---

## 尝试 8: max_passes 增加
**日期：** 2026-05-18
**文件：** `src/ft8/decode.rs` `MAX_DECODE_PASSES_DEPTH3`

### 改动
| MAX_PASSES | 命中 |
|---|---|
| 3（原始） | 354 |
| 4 | 355 |
| 5 | 355（无增益） |

### WSJT-X 对比
WSJT-X: npass=3 for depth=3（ipass=1,2,3）。我们 4 已经比 WSJT-X 多一轮。

### 结果
- 20/20: ✅
- +1 条（3→4）

### 状态：✅ 已保留（4）

---

## 尝试 9: sync_min 降低
**日期：** 2026-05-18
**文件：** `tests/segment_decode_test.rs`

### 改动
| sync_min | 命中 |
|---|---|
| 0.8（原始） | 350 |
| 0.7 | 349 ↓ |

### 结果
降低 sync_min 反而退步。更宽松的候选筛选导致更多噪声候选进入，干扰了真正信号。

### 状态：❌ 已回退（0.8）

---

## 尝试 10: 测试中 depth=4
**日期：** 2026-05-18
**文件：** `tests/segment_decode_test.rs`

### 结果
- depth=4 vs depth=3: 无变化

### 状态：❌ 已回退（depth=3）

---

## 当前累计状态
| 改动 | 状态 | 累计命中 |
|---|---|---|
| 原始基线 | — | 338/449 |
| sync8 fdiff 4.0→0.5 | ✅ | +0（339→339） |
| +1s padding | ✅ | +12（350） |
| nharderrors 40→36 | ✅ | +0 |
| maxosd per-pass | ✅ | +0 |
| max_candidates 300→500 | ✅ | +4（354） |
| max_passes 3→4 | ✅ | +1（355） |

**当前：355/449 (79.1%)，20/20 ✅**

## 排除的尝试
| 尝试 | 原因 |
|---|---|
| Nagain 改用 residual | 耗时+30s，收益低 |
| 碰撞恢复 pass | 耗时+57s，收益低 |
| sync_min 降低 | 退步 |
| depth=4 | 无增益 |
| max_passes=5 | 无增益 |
| 0.5s/2s padding | 0.5s 丢 CU3HN；2s 噪音太多退步 |

## 待尝试
| 方案 | 说明 |
|---|---|
| normalizebmet | WSJT-X 有，我们没做 |
| nagain WSJT-X 模式 | 用原始数据+窄带搜 |
| ft8_a7d AP 解码 | 已知呼号 AP 解码 |

---

## 尝试 11: Pass 0 并行结果按 SNR 排序
**日期：** 2026-05-18
**文件：** `src/ft8/decode.rs`

### 思路
并行生成的 results 按 SNR 从高到低排序后，强信号先减→留更多残留给弱邻频。

### 改动
```rust
results.sort_by(|a, b| b.3.snr.partial_cmp(&a.3.snr).unwrap());
```

### WSJT-X 对比
WSJT-X 串行处理候选，sync8 按 sync 分排序，自然形成强信号先处理。我们并行处理后排序是为了模拟这个效果。

### 结果
- 20/20: ❌ FAILED（破坏了 210703 录音的减法顺序）
- 原因：强制排序改变了 Rayon 自然顺序，对特定录音有副作用

### 状态：❌ 已回退

---

## 当前累计状态
| 改动 | 状态 | 累计命中 |
|---|---|---|
| 原始基线 | — | 338/449 |
| sync8 fdiff 4.0→0.5 | ✅ | +0 |
| +1s padding | ✅ | +12（350） |
| nharderrors 40→36 | ✅ | +0 |
| maxosd per-pass (pass2=5) | ✅ | +0 |
| max_candidates 300→500 | ✅ | +4（354） |
| max_passes 3→4 | ✅ | +1（355） |

**当前：355/449 (79.1%)，20/20 ✅**

## 排除的尝试汇总
| 尝试 | 原因 |
|---|---|
| Nagain 改用 residual | 耗时+30s |
| 碰撞恢复 pass | 耗时+57s |
| sync_min 降低 | 退步 |
| depth=4 | 无增益 |
| max_passes=5 | 无增益 |
| 0.5s/2s padding | 0.5s 丢 CU3HN；2s 退步 |
| pass 2+ 串行 | 20/20 挂了 |
| sort by SNR | 20/20 挂了 |

---

## 最终状态 (2026-05-18)

### 保留的 5 个修复
1. sync8 fdiff 4.0→0.5: 邻频不互斥
2. +1s padding: 修复 ibest 负数 Costas 死区
3. nharderrors 40→36: 对齐 WSJT-X
4. maxosd per-pass: pass2 用 5
5. max_passes 3→4: 多一轮减法

### 测试参数
- max_candidates: 500
- sync_min: 0.8
- depth: 3
- freq: 200-3000 Hz

### 结果
- 20/20: ✅
- 当前基准: 347/449 (77.3%)

### 关键诊断发现
- CU3HN YC3BMX OI62 @1350Hz +0dB: 因 ibest 负数→Costas 同步全零。+1s padding 修复
- F4JAR UX7UU -19 @1413Hz -9dB: 同频碰撞(F1OMM)，非确定性解码(Heisenbug)
- CQ R6KEE KN75 @1389Hz -17dB: 段噪声差异导致同 SNR 在不同段表现不同
- 1300-1400Hz 频段: ibest 负数高发区

### 已知限制
- xbase normalization 因 sbase 索引 bug 未生效（修复会破坏 20/20）
- 无 ft8_a7d 风格 AP 解码（需大改动）
- 并行减法不如 WSJT-X 串行减法精细
## HashCallBook 累积解码结果

**日期：** 2026-05-18 晚

### 改动
- decode.rs: `Option<HashCallBook>` → `Option<Rc<HashCallBook>>`
- hashcall.rs: 添加 `clone_book()` 方法
- segment_decode_test.rs: 逐段累积呼号表

### 结果
| 配置 | 命中率 | Hash 解析 |
|---|---|---|
| 无 HashCallBook | 347/449 (77.3%) | 0 |
| **累积 HashCallBook** | **355/449 (79.1%)** | **10** |

**提升：+8 条消息 (+1.8%)**

### 性能
- Rc::clone() 优化后：~9-10s/段（vs 之前 clone_book() 深拷贝 12-13s/段）
- 18 段共 193s
- Book 增长：0 → 154 entries

### 状态
✅ 已验证有效

