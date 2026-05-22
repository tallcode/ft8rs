# WSJT-X 对齐改造清单

## 基线对比

| 测试 | FFTW@3840 | rustfft@4096 | WSJT-X |
|---|---|---|---|
| 短解码 | 19/20 | 20/20 | 20/20 |
| 长解码 | 358/449 (79.7%) | 353/449 (78.6%) | ~420+/449 |

## 已验证为零增益的改造 ✅

- [x] FFT 引擎双引擎切换（FFTW@3840 / rustfft@4096）— 灵敏度无差异
- [x] maxosd pass 1 depth=2 — 358/449 vs 358/449，零增益
- [x] xsnr2 fallback — 358/449 vs 358/449，零增益
- [x] SYNC8_DF 对齐 (3.125 Hz/bin)
- [x] subtract_ft8 共轭对称修复 (fft_r2c → fft_complex)
- [x] xbase LLR 归一化
- [x] SNR 门控: nsync≤10 && snr<-24dB → 拒绝

## 当前基线

| 模式 | 短解码 | 长解码 |
|---|---|---|
| ft8rs (FFTW@3840) | 19/20 | 358/449 (79.7%) |
| WSJT-X 参考 | 20/20 | ~366/449+ |
| 差距 | 1 | ~8+ |

## 待调查 ⚠️

### 1. sync8 sync 值精度
sync2d 计算中 t/t0 的浮点精度可能导致弱信号 sync 值在 syncmin=1.3 门限上下波动。

### 2. subtract_ft8 质量
减法后残差质量影响后续 pass。需要对比 WSJT-X subtractft8.f90 的 LPF 参数。

### 3. LLR 数值精度
normalize_bmet 的浮点差异累积可能影响边际信号的 BP 收敛。

## 待对齐 ⚠️

### 2. SNR 计算 xsnr2 fallback ~~(中优先级)~~ ✅ 已对齐（零增益）
**基准测试**: 358/449 vs 358/449 — **零增益**

xsnr2 只改变 SNR 报告方式，不影响解码决策。已实现但
不带来匹配数提升。

### 1. maxosd pass 1 depth=2 ~~(高优先级)~~ ✅ 已对齐（零增益）
**WSJT-X ft8_decode.f90**: pass 1 用 `ndeep=2`（→ maxosd=0），passes 2-3 用 `ndeep=3`（→ maxosd=2）
**我们**: 已对齐，pass 1 depth=2, passes 2-3 depth=3

**基准测试**: 358/449 vs 358/449 — **零增益**

WSJT-X pass 1 更保守（maxosd=0 避免假阳性），passes 2-3 在
清理后残差上用 maxosd=2 补偿。总结果和所有 pass 用 maxosd=2
一样，因为减法会清理假阳性。

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
