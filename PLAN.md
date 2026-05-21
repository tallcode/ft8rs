# FT8 流式解码开发计划

## 当前状态

### 已完成
- [x] 基础解码架构对齐 (sync8 → ft8b → subtract)
- [x] 12kHz 采样率, 15s 时间窗
- [x] 3 pass decode, syncmin=1.3
- [x] 并行候选解码 (hash_call_book=None)
- [x] StreamDecoder / AudioBuffer / CrossSlotMemory 骨架
- [x] ft8b_stream 独立实现
- [x] 性能达标: 短解码 1.9s, 长解码每段 1.3-2.5s

### 灵敏度瓶颈
- 短解码: 20/20 ✅
- 长解码: **217/449 (48%)** ❌ 目标 360/449 (80%)
- 差距约 **143条消息**

---

## 根因分析

对比 wsjtx/lib/ft8_decode.f90，当前缺失的关键功能：

### 1. 渐进式解码 (nzhsym 41/47/50) — 高优先级
**WSJT-X 行为:**
- nzhsym=41 (~11s): syncmin=2.0 强信号早期解码 → 保存到 ndec_early
- nzhsym=47 (~13s): 对 ndec_early 信号做 subtractft8 (lrefinedt) → 清理残差
- nzhsym=50 (15s): 在清理后的残差上做 3-pass full decode

**当前:** 一次性对整个 15s 做 full decode，无早期捕获/减法

**影响:** 强信号不 subtraction 会掩盖同频附近的弱信号，预计损失 30-50 条

### 2. ft8_a7d AP 解码 — 高优先级
**WSJT-X 行为:**
- 保存上一时隙的解码结果到 `ndec(jseq,0)` (dt/freq/"call_1 call_2")
- 在当前时隙 nzhsym=50 时，对每个上一时隙的频率/时间位置跑 ft8_a7d
- ft8_a7d **不是 LDPC 解码**，而是暴力枚举 206 种可能的消息变体（基于已知 callsign 组合），计算 Hamming 距离选最优
- 验证: dmin < 100.0 且 dmin2/dmin > 1.3

**当前:** ap_decode.rs 只是重新跑 ft8b_stream，完全不是 WSJT-X 的算法

**影响:** 已知.callsign 的弱信号无法被强制解码，预计损失 80-100 条

### 3. 跨时隙 HashCallBook 共享 — 中优先级
**WSJT-X 行为:**
- 每个时隙解码后通过 ft8_a7_save 保存 "call_1 call_2" 对
- HashCallBook 在整个传输过程中累积 callsigns
- 下一时隙的 unpack77 可以用 book 解析 `<...>` 哈希调用

**当前:** StreamDecoder 有 book 但只在 decode_slot 后简单提取 callsigns，没有跨时隙传递 AP 信息

### 4. long_decode.rs 使用 Amplitude 模式 — 中优先级
**WSJT-X 行为:** 所有 pass 使用 Power 模式 sync8

**当前:** progressive_decode 用了 Amplitude + Power 两种模式

---

## 开发计划

### Phase 1: 渐进式解码 (nzhsym 41/47/50)

**文件:** `src/stream/decoder.rs`, `src/stream/buffer.rs`

1. **decode_slot 改为 progressive_decode_slot**
   - 输入: 完整 15s 12kHz 音频
   - nzhsym=41: 取前 11s, syncmin=2.0, depth=3 → 强信号 early decode
   - nzhsym=47: 对 early decode 结果做 subtract (lrefinedt=true, dt<0.396 才减)
   - nzhsym=50: 在 subtracted residual 上做 full decode (syncmin=1.3, depth=3)
   - 合并所有解码结果 (early + full)

2. **早期解码结果保存到 CrossSlotMemory**
   - 保存 freq, dt, msg, itone, snr, sync
   - 标记 subtracted 状态

3. **减法逻辑复用**
   - 复用 `subtract_ft8_refined` from `ft8/subtract.rs`
   - 条件: dt < 0.396 (WSJT-X 的 xdt_save(i)-0.5.lt.0.396)

### Phase 2: ft8_a7d AP 解码

**文件:** 新建 `src/util/ap_decode.rs` (替换 `src/stream/ap_decode.rs`)

1. **实现 ft8_a7d 算法** (完全对齐 wsjtx/lib/ft8/ft8_a7.f90:ft8_a7d)
   - 输入: dd0 (15s 音频), call_1, call_2, grid4, xdt, f1, xbase
   - 下采样到基带 (ft8_downsample)
   - 时间对齐 ±10, 频率对齐 ±2.5Hz
   - 提取软符号 (79 symbols × 8 tones)
   - 构建 bit metrics (bmeta/bmetb/bmetc/bmetd)
   - **暴力枚举 206 条消息**:
     - 对每个 imsg=1..206: 组合 call_1 + call_2 + 后缀 (RRR/RR73/73/grid/SNR/CQ)
     - genft8 → 编码为 77-bit + 174-bit codeword
     - 计算 Hamming 距离: da/db/dc/dd (对 4 组 LLRs)
     - 取 min 作为 dmm(imsg)
   - 验证: dmin < 100.0 且 dmin2/dmin > 1.3
   - 输出: msg37, xsnr, nharderrors

2. **消息枚举逻辑** (206 条)
   - 基于 pack_jt77.rs 的组合逻辑
   - std_call1/std_call2 判断 (callsign 是否标准)
   - 后缀: RRR, RR73, 73, grid, CQ, SNR reports (+/-)

3. **需要的新函数**
   - `genft8(msg) → (msgsent, msgbits, itone)` — 消息到 77-bit + tones
   - `stdcall(call) → bool` — callsign 标准化检查

### Phase 3: 跨时隙 AP 集成

**文件:** `src/stream/decoder.rs`, `src/stream/cross_slot.rs`

1. **跨时隙记忆数据结构**
   ```
   PreviousSlotDecode {
       call_1: String,
       call_2: String,
       grid4: String,  // 4-char grid or "RR73"/"+10"/"-"
       dt: f64,
       freq: f64,
   }
   ```

2. **decode_slot 流程更新**
   - 解码完成后: 解析消息提取 call_1/call_2/grid4 → 保存到 previous_slot
   - 下一时隙: 对 previous_slot 每个条目跑 ft8_a7d
   - 合并 AP decode 结果

3. **HashCallBook 共享**
   - 单个 StreamDecoder 实例跨时隙复用
   - book 累积 callsigns 用于 `<...>` 解析

### Phase 4: long_decode.rs 修正

1. **移除 Amplitude 模式** — 全部使用 Power 模式
2. **集成渐进式解码** — 使用 Phase 1 的逻辑
3. **集成跨时隙 AP** — 使用 Phase 3 的逻辑

### Phase 5: 测试验证

1. **短解码测试:** 210703_133430.wav → ≥19/20, <15s
2. **长解码测试:** 230208_140300.wav → ≥360/449, 每段 <15s
3. **超时机制:** 单段 >15s 终止
4. **灵敏度校验:** 基线 ±10 范围外终止

---

## 关键设计决策

1. **AP decode 用暴力枚举，不用 LDPC** — 完全对齐 WSJT-X ft8_a7d
2. **渐进式解码 per-slot 内部完成** — 不依赖外部时序，stream decoder 接收完整 15s 后内部模拟 nzhsym 推进
3. **跨时隙 AP 只在 nzhsym=50 后执行** — 对齐 WSJT-X
4. **永远传 hash_call_book=None 给 decode_ft8** — 保持并行候选解码

## 预估影响

| 改进 | 预计增益 |
|------|---------|
| 渐进式解码 (subtract) | +30-50 条 |
| ft8_a7d AP decode | +80-100 条 |
| 跨时隙 HashCallBook | +20-30 条 (含 <...> 解析) |
| **总计** | **+130-180 条** |

目标: 217 → **350-400/449**

## 反思 (阅读 TRY.md 后)

- 不要偏离 WSJT-X 架构做性能优化
- 不要在未对齐前先测试
- 测试必须 release + 超时15s + 灵敏度校验
- AP 贡献不会特别大 (baseline ±10 以内), 重点是渐进式减法
