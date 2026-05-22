# WSJT-X 对齐改造清单

## 基线对比

| 测试 | FFTW@3840 | rustfft@4096 | WSJT-X |
|---|---|---|---|
| 短解码 | 19/20 | 20/20 | 20/20 |
| 长解码 | 358/449 (79.7%) | 353/449 (78.6%) | ~420+/449 |

## 已完成 ✅

- [x] FFT 引擎双引擎切换（FFTW@3840 / rustfft@4096）
- [x] FFT 尺寸对齐 (3840 via FFTW)
- [x] SYNC8_DF 对齐 (3.125 Hz/bin)
- [x] subtract_ft8 共轭对称修复 (fft_r2c → fft_complex)
- [x] xbase LLR 归一化启用
- [x] SNR 门控: nsync≤10 && snr<-24dB → 拒绝
- [x] 单元测试全部通过

## 待对齐 ⚠️

### 1. maxosd 条件逻辑 ~~(高优先级)~~ ✅ 已对齐
**WSJT-X**: `maxosd=2` 默认值就是 2，条件成立也是 2
**我们**: maxosd_base=2 ✅ 一致

TODO 原始分析有误 — WSJT-X 的 maxosd 初始值就是 2，
条件判断只是保持 2 而不是改成其他值。standalone ndepth=3
模式下两者完全一致。

### 2. SNR 计算 xsnr2 fallback ~~(中优先级)~~ ✅ 已对齐
**基准测试**: 358/449 vs 358/449 — **零增益**

xsnr2 只改变 SNR 报告方式，不影响解码决策。已实现但
不带来匹配数提升。

### 3. AP 解码参数体系 (中优先级)
WSJT-X ft8b.f90 有大量参数控制 AP 行为：
- `nQSOProgress`: QSO 进度状态 (0-5)
- `iaptype`: AP 解码类型 (1=CQ, 2=MyCall, 3=MyCall+DxCall, 4/5/6=RRR/73/RR73)
- `ncontest`: 竞赛模式 (影响 AP 掩码)
- `apsym`/`aph10`: AP 符号模式
- `nappasses`/`naptypes`: AP pass 数量和类型矩阵

我们的 AP 解码（ft8_a7d）只实现了部分类型，缺少:
- QSOProgress 状态跟踪
- 竞赛模式 AP 掩码 (mcq/mcqfd/mcqru/mcqww 等)
- Hound/Fox 模式
- AP pass 数量动态调整

### 4. nzhsym 渐进式符号控制 (低优先级)
**WSJT-X**: `if(nzhsym<50) npasses=4`（渐进模式无 AP）
**我们**: 总是做 4 passes

WSJT-X 在使用截断音频（nzhsym<50）时不做 AP，避免假阳性。

### 5. 假阳性过滤增强 (低优先级)
WSJT-X 在 ft8b 结尾有额外检查:
```fortran
if(nsync.le.10 .and. xsnr.lt.-24.0) then
    nbadcrc=1
    return
endif
```
我们已实现类似逻辑，但 WSJT-X 可能还有更多隐含过滤。

### 6. 架构对齐 (长期)
- `ft8c.f90`: 已知呼号辅助解码 (call_1, call_2, grid4)
- `ft8d.f90`: 顶层解码协调器
- 跨时隙记忆与 WSJT-X ndec(jseq,0) 完全对齐

## 优先级排序

1. **maxosd 条件逻辑** — 直接减少假阳性，可能 +5~10 条
2. **xsnr2 fallback** — 改善边际信号 SNR 估计，可能 +3~5 条
3. **AP 参数体系** — 完善已知呼号辅助解码，可能 +5~10 条
4. **其余** — 边际优化

## 预估总收益

| 改进项 | 预估增益 |
|---|---|
| maxosd 条件 | +5~10 |
| xsnr2 | +3~5 |
| AP 完善 | +5~10 |
| 其他 | +2~5 |
| **合计** | **+15~30** |

目标: 358 → 388~408 (仍距 WSJT-X 420+ 有 12~32 条差距，
可能需要更深层的数值精度优化)
