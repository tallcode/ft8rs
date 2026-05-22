# FT8 流式解码开发计划

## 当前状态 (2026-05-21)

### 已对齐 WSJT-X
| 模块 | WSJT-X | ft8rs | 状态 |
|------|--------|-------|------|
| sync8 | sync8.f90 | ft8/decode.rs (sync8 fn) | ✅ |
| ft8b | ft8b.f90 | ft8/decode.rs (ft8b fn) | ✅ |
| 三 pass | ft8_decode.f90 | ft8/decode.rs (decode fn) | ✅ |
| subtractft8 | subtractft8.f90 | util/subtract_ft8.rs | ✅ |
| LDPC BP+OSD | decode174_91.f90 | util/decode174_91.rs | ✅ |
| HashCallBook | hash.f90 | util/hashcall.rs | ✅ |
| 下采样 | ft8_downsample.f90 | ft8/decode.rs | ✅ |

### 测试结果
| 测试 | 目标 | 当前 | 状态 |
|------|------|------|------|
| 短解码 | ≥19/20, <15s | 20/20, 4.1s | ✅ |
| 长解码 | ≥366/449, 每段<15s | 351/449 (78.2%), 79s | ⚠️ 差15条 |

### 核心瓶颈
当前解码是一次性对 15s 音频跑 3 pass decode。**WSJT-X 是渐进式的**：
1. nzhsym=41 (~11s): syncmin=2.0 早期解码 → 保存强信号
2. nzhsym=47 (~13s): 对早期信号做 subtract (仅 dt<0.396) → 清理残差
3. nzhsym=50 (15s): 在干净残差上做 3 pass full decode
4. nzhsym=50 后: 对上一时隙的解码结果跑 ft8_a7d AP 解码

**预计增益**: 渐进式减法 +30-50条, AP解码 +5-15条

---

## Phase 1: 渐进式解码 (nzhsym 41/47/50)

### 目标
实现 WSJT-X ft8_decode.f90 的渐进式解码架构

### 设计
```
decode_slot(samples: 15s @ 12kHz) → Vec<DecodedMessage>
  │
  ├── Stage 1: nzhsym=41, 前 11s (41×270=11070 samples)
  │     syncmin=2.0, depth=3
  │     → early_decodes (强信号)
  │
  ├── Stage 2: nzhsym=47, 前 13s
  │     对 early_decodes 中 dt<0.396 的信号做 subtract_ft8_refined
  │     → cleaned_audio (前13s被清理)
  │
  └── Stage 3: nzhsym=50, 完整 15s
        在 cleaned_audio 上做 full decode (syncmin=1.3, depth=3)
        → full_decodes
        合并 early + full 结果, 去重
```

### 关键细节 (对齐 wsjtx/lib/ft8_decode.f90)
- **nzhsym=41 截断**: `n = 41 * NSPS = 41 * 270 = 11070 samples` (~11s)
  - 实际上 WSJT-X 传的是 `nzhsym*NSPS` 个 samples 给 sync8
  - sync8 内部: `NHSYM = NMAX/NSTEP - 3 = 372`, 但 nzhsym 限制了有效符号数
  - 我们简化为：取前 `nzhsym * NSTEP` samples 传给 sync8
- **nzhsym=47 减法条件**: `xdt_save(i) - 0.5 < 0.396` → 只对 dt 小的信号减
  - `lrefinedt = (ndepth > 2)` → ndepth=3 时为 true
  - subtractft8 的 lrefinedt 参数控制是否做 ±90 样本的时间精炼
- **nzhsym=50 残差填充**: 
  ```fortran
  n = 47 * 3456  ! 47*NSPS*NDOWN = 47*270*60? 不对, 看实际代码
  dd(1:n) = dd1(1:n)  ! 用已清理的前面部分
  dd(n+1:) = iwave(n+1:)  ! 用原始音频的后面部分
  ```
  实际上 n=47*NSPS=12690 个 samples? 需要仔细对齐。

### 实现文件
- `src/stream/decoder.rs`: 重写 `decode_slot` → `progressive_decode_slot`
- 复用现有的 `decode()` 函数, 但需要控制输入音频长度和 syncmin

### 验收
- 短解码: ≥19/20, <15s
- 长解码: 灵敏度提升 ≥+15条

---

## Phase 2: ft8_a7d AP 解码

### 目标
实现 WSJT-X ft8_a7d 算法 (暴力枚举 206 种消息变体)

### 算法 (对齐 wsjtx/lib/ft8/ft8_a7.f90:ft8_a7d)
```
ft8_a7d(dd0, call_1, call_2, grid4, xdt, f1, xbase) → (msg37, xsnr, nharderrors)
  │
  ├── 下采样到基带 (ft8_downsample)
  ├── 时间对齐 ±10 → ibest
  ├── 频率对齐 ±2.5Hz → delfbest
  ├── 二次下采样
  ├── 时间精炼 ±4
  ├── 提取软符号 (79 symbols × 8 tones)
  ├── sync gate: nsync > 6 才继续
  ├── 构建 bit metrics (bmeta/bmetb/bmetc/bmetd, normalize)
  ├── scalefac=2.83 → LLRs
  │
  └── 暴力枚举 206 条消息:
        for imsg=1..206:
          组合 call_1 + call_2 + 后缀 (RRR/RR73/73/grid/CQ/SNR)
          genft8 → msgbits, itone, msgsent
          encode174_91 → cw (174-bit codeword)
          对 4 组 LLRs 算 Hamming 距离: da, db, dc, dd
          dmm(imsg) = min(da, db, dc, dd)
        dmin = min(dmm), dmin2 = second min
        验证: dmin < 100.0 AND dmin2/dmin > 1.3
        → msg37, xsnr, nharderrors
```

### 206 种消息枚举 (对齐 ft8_a7.f90:140-200)
```
imsg=1: <call_1> call_2 RRR      (非标准call_1)
imsg=2: call_1 call_2 RRR
imsg=3: call_1 call_2 RR73
imsg=4: call_1 call_2 73
imsg=5: CQ call_2 [grid4]        (标准call_1, grid4可能是RR73)
imsg=6: call_1 call_2 grid4
imsg=7-206: SNR reports (-50到+52, 奇偶分+/-)
```

### 需要的函数
- `genft8(msg) → (msgsent, msgbits, itone)` — 消息编码
- `stdcall(call) → bool` — callsign 标准化检查 (已有 pack77 可复用)
- `encode174_91(msgbits) → cw` — LDPC 编码 (已有)

### 实现文件
- 新建 `src/util/ap_decode.rs` (替换旧的 ap_decode.rs)
- 新建 `src/util/genft8.rs` (消息编码)

### 验收
- AP 解码在基线测试上贡献 ≥+5条
- 单条 AP 解码 < 100ms

---

## Phase 3: 跨时隙记忆 + HashCallBook 共享

### 目标
实现 WSJT-X 的跨时隙 AP 信息传递

### 设计
```
struct PreviousSlotDecode {
    call_1: String,
    call_2: String,
    grid4: String,    // 4-char grid / "RR73" / "+10" / "-" / "    "
    dt: f64,
    freq: f64,
    xbase: f64,
}

struct CrossSlotMemory {
    previous: Vec<PreviousSlotDecode>,  // 上一时隙的解码结果
    jseq: usize,                         // 0=even, 1=odd sequence
}
```

### 流程
```
decode_slot(samples) → Vec<DecodedMessage>
  │
  ├── 1. 渐进式解码 (Phase 1) → current_decodes
  │
  ├── 2. AP 解码 (Phase 2):
  │     for prev in memory.previous:
  │       ft8_a7d(samples, prev.call_1, prev.call_2, prev.grid4, 
  │                prev.dt, prev.freq, prev.xbase)
  │       → 如果 nharderrors >= 0, 加入 current_decodes
  │
  ├── 3. 解析 current_decodes, 提取 call_1/call_2/grid4
  │     → 更新 memory.previous (为下一时隙准备)
  │
  └── 4. 去重, 返回
```

### HashCallBook 共享
- 单个 StreamDecoder 实例跨时隙复用
- book 累积 callsigns 用于 `<...>` 解析
- 已有的 HashCallBook 实现只需确保跨 slot 传递

### 验收
- 长解码灵敏度提升 ≥+5条 (含 `<...>` 解析增益)

---

## Phase 4: 测试验证

### 测试约束
- **全部 release 模式**
- **单段解码超时 15s** → 失去流式意义, 提前终止
- **灵敏度校验**: 基线 ±10 范围外 → 严重问题, 提前终止

### 测试用例
1. **短解码**: 210703_133430.wav → ≥19/20, <15s
2. **长解码**: 230208_140300.wav → ≥366/449, 每段<15s

### 运行命令
```bash
cargo test --release test_stream_decode_short_audio -- --nocapture
cargo test --release test_stream_decode_long_audio -- --nocapture
```

---

## 关键设计决策

1. **渐进式解码 per-slot 内部完成** — stream decoder 接收完整 15s 后内部模拟 nzhsym 推进
2. **AP decode 用暴力枚举** — 完全对齐 WSJT-X ft8_a7d, 不是重新跑 LDPC
3. **跨时隙 AP 只在 nzhsym=50 后执行** — 对齐 WSJT-X
4. **始终传 hash_call_book=None 给 decode()** — 保持并行候选解码
5. **AP 贡献预期不大** — baseline ±10 以内, 重点是渐进式减法

## 预估影响

| 改进 | 预计增益 | 优先级 |
|------|---------|--------|
| 渐进式解码 (subtract) | +30-50 条 | 🔴 高 |
| ft8_a7d AP decode | +5-15 条 | 🟡 中 |
| 跨时隙 HashCallBook | +5-10 条 | 🟡 中 |
| **总计** | **+40-75 条** | |

目标: 351 → **390-425/449**
