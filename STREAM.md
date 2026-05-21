# STREAM — WSJT-X 流式解码对齐文档

## WSJT-X 完整解码流程

### 音频参数
| 参数 | 值 | 说明 |
|------|-----|------|
| 采样率 | **12000 Hz** | 内部采样率，声卡可能48kHz但内部重采样到12kHz |
| 位宽 | **16-bit integer (i\*2)** | `integer\*2 iwave(15*12000)` |
| 时隙长度 | **15秒** | 180000 samples |
| 频率范围 | **200-3000 Hz** (默认) | nfa=200, nfb=3000 |

### 渐进式解码窗口 (nzhsym)
| nzhsym | 音频量 | 动作 |
|--------|--------|------|
| < 41 | < 11s | 等待（不解码）|
| 41 | ~11s | Early: sync8(syncmin=2.0) → ft8b → ndec_early |
| 47 | ~13s | Subtract: ndec_early + lrefinedt (仅 dt<0.396) |
| 50 | 15s | Full: sync8(syncmin=1.3) → 3-pass on cleaned residual |

### Full decode (nzhsym=50) 三 pass 架构
```
npass=3 (ndepth=3)
do ipass=1,npass:
    newdat = .true.           ← 每次都重置,触发 ft8_downsample 重算
    syncmin = 1.3             ← 各 pass 相同
    sync8(dd, ...) → candidates
    do icand=1,ncand:         ← 顺序处理
        ft8b(dd, newdat, ...) → msg37/xsnr/itone
        if nbadcrc==0: 保存、subtract
    enddo
    if ipass==2 .and. ndecodes==0: cycle
enddo
```

### AP (ft8_a7d) 解码
- 在 nzhsym=50 三 pass 后执行
- 使用上一时隙的解码结果: call_1, call_2, grid4, xdt, f1
- 暴力枚举 206 种消息变体 → 最小 Hamming 距离
- 验证: dmin<100, dmin2/dmin>1.3

## ft8b 子流程
```
ft8b(dd0, newdat, f1, xdt) → msg37, xsnr, itone
  ft8_downsample(dd0, f1) → cd0
  时间对齐 ±10
  频率对齐 ±2.5 Hz
  ft8_downsample refined
  时间精炼 ±4
  软符号提取 (32-point FFT, /1000)
  sync gate: nsync >= 7
  bit metrics → normalize_bmet → LLRs
  npasses=4:
    BP+OSD (scalefac=2.83)
    if valid → unpack77 → msg37
  SNR 估计: xsnr2 (xbase based)
  若成功: subtractft8(dd0, ...)
```

## 当前实现

### 已对齐
| 模块 | WSJT-X | ft8rs | 状态 |
|------|--------|-------|------|
| sync8 | sync8.f90 | ft8/decode.rs (sync8 fn) | ✅ 40th percentile per-bin 归一化 |
| ft8b | ft8b.f90 | ft8/decode.rs (ft8b fn) | ✅ 时间/频率对齐、gate、bit metrics、LDPC |
| 三 pass | ft8_decode.f90 | ft8/decode.rs (decode fn) | ✅ 顺序候选、即时 subtract、FFT 重算 |
| decode174_91 | decode174_91.f90 | util/decode174_91.rs | ✅ BP+OSD, nharderrors<=36 |
| subtractft8 | subtractft8.f90 | util/subtract_ft8.rs | ✅ FFT 线性卷积、LPF cos² |
| ft8_a7d (AP) | ft8_a7.f90 | util/ap_decode.rs | ✅ 206-variant brute-force |
| downsample | ft8_downsample.f90 | ft8/decode.rs | ✅ taper + shift + IFFT |
| HashCallBook | hash.f90 | util/hashcall.rs | ✅ 跨 slot Rc 累积 |

### 架构决策
- **顺序候选处理**: 匹配 WSJT-X 单线程 `do icand=1,ncand`
- **即时减法**: 每个成功解码后 `subtractft8(dd, ...)` 并重算 FFT，下一候选看到清理后信号
- **HashCallBook 跨 slot**: `Rc<HashCallBook>` 在 StreamDecoder 各 slot 间累积
- **移除 JTDX 风格**: 删除 nagain pass、SyncMode 多模式、pass syncmin 递减

## 测试

| 测试 | 音频 | 目标 | 实际 |
|------|------|------|------|
| 短解码 | 210703_133430.wav (12kHz 16-bit) | ≥19/20, <15s | **20/20, 4.1s** ✅ |
| 长解码 | 230208_140300.wav (48kHz→12kHz) | ≥316/449 (70%), 每段<15s | **351/449 (78.2%), 79s total** ✅ |

### 长解码测试细节
- 18 segment, 每个 17s 窗口 (15s ±1s overlap)
- 每段 3.3-6.5s, 全部 ≤15s ✅
- 顺序处理 (匹配实时流式), 非并行段

### 运行命令
```bash
cargo test --release test_stream_decode_short_audio -- --nocapture   # 20/20, 4.1s
cargo test --release test_stream_decode_long_audio -- --nocapture    # 351/449, 79s
```
