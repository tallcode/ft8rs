# FT8 灵敏度差异全面分析

> 基准：ft8rs 355/449 (79.1%) vs WSJT-X/JTDX 交叉验证 ~420-430/449 (93-96%)
> 差距：约 65-75 条消息未解码

---

## 一、总体差异概览

| 维度 | WSJT-X | JTDX | ft8rs | 影响 |
|------|--------|------|-------|------|
| sync8 计算方式 | 1种（功率） | 3种（sqrt/power/abs） | 1种（功率） | **高** |
| 同步质量门控 | nsync≤6→放弃 | 多层 SNR-based 门控 | nsync≤3→放弃 | **高** |
| 频谱基线 | 40%分位，归一化red | 同WSJT-X | 40%分位，归一化 | 中 |
| 多pass策略 | 3 passes + nagain | 3 cycles × 3-10 passes | 4 passes + nagain | **高** |
| 跨时隙信号复用 | a7 解码 | even/odd signals + virtual QSO | 无 | **高** |
| 信号减法 | 减→nagain | 减→多轮减→nagain | 减→nagain | 中 |
| AP解码 | 完整 (iaptype 1-6) | 超完整 (含CQ/mycall/DX信号) | 框架（Pass 9/10未启用） | **高** |
| SNR估算 | sbase+signal ratio | 双重SNR + sbase | 简化SNR | 中 |
| 下调采样 taper | cos² 800点 | cos² + 边缘增益 + 高灵敏模式 | cos² 101点 | 低-中 |
| 跨段累积 | a7 | even/odd 信号记忆 | HashCallBook（只呼号） | 中 |

---

## 二、sync8 层面的差异（最重要）

### 2.1 sync 计算方式

**JTDX** 在 9 个 pass 中使用 3 种不同的 sync 计算：

```
pass 1,4,7: s(i,j) = sqrt(Re² + Im²)     ← 幅度谱
pass 2,5,8: s(i,j) = Re² + Im²            ← 功率谱
pass 3,6,9: s(i,j) = |Re| + |Im|          ← 绝对值谱
```

**ft8rs** 只用一种：功率谱 (`s = Re² + Im²`)。

**影响**：不同的谱表示对弱信号的 SNR 放大效果不同。sqrt 对弱信号更友好（压缩动态范围），abs 对脉冲噪声更鲁棒。JTDX 通过 3 轮不同计算方式覆盖更多信号类型。

### 2.2 同步质量检查（最关键的差距）

**ft8rs** 只用简单的硬同步计数：
```rust
nsync = is1 + is2 + is3;  // 3个Costas块，每块7个符号，max=21
if nsync <= 3 { bail }    // 阈值只有3！
```

**JTDX** 使用多层 SNR-based 同步评分系统：

1. **nsyncscore**：每个符号检查信噪比 `synck > sumk`（sync tone > 平均噪声），统计通过数（max=21）
2. **scoreratio**：`synck/sumk` 的加权平均
3. **nsyncscorew**：特殊加权评分（用整体 s8 而非单符号）
4. **scoreratiowa**：加权平均比率
5. **nsync2**：第二候选 Costas 匹配数
6. 对 CQ 候选有额外的 rscq 评分
7. 多层门控（~20 个不同条件分支），根据 dt 范围（前/中/后段）不同

**JTDX 的门控逻辑示例**（ft8rs 完全没有这个）：
```
if nsyncscore < 8 || (nsyncscore < 10 && scoreratio < 5.5) → bail
if nsyncscore == 11 && scoreratio < 5.37 && nsyncscore1 < 5 && nsyncscore3 < 5 → bail
if nsyncscorew < 3 → bail (除非个别块超强)
```

**影响**：JTDX 不会轻易因为 nsync 低就放弃——即使 nsync=4，如果每个 sync 符号的 SNR 很高（scoreratio 高），它也会继续尝试解码。ft8rs 的 nsync≤3 门控太过激进，很多边缘信号在第一关就被丢弃。

### 2.3 频谱基线（sbase）

**ft8rs** 计算 sbase 的方式：
- FFT 用 `next_pow2(NFFT1)` = 4096 点（`df = 2.93 Hz/bin`）
- 在 sync8 中用于归一化 `red`

**WSJT-X/JTDX** 计算 sbase 的方式：
- FFT 用 NFFT1 = 3840 点（`df = 3.125 Hz/bin`）
- 不同分辨率导致频点索引不同

**关键差异**：ft8rs 的 sync8 用 4096-point FFT，而 ft8b 的 sbase 访问用 `DOWNSAMPLE_DF = 0.0625 Hz/bin` 做索引。这个索引不匹配已经在 TRY.md 中记录（sbase 索引 bug），导致归一化对 f > 128Hz 的信号完全无效。

### 2.4 候选去重

**WSJT-X**：`fdiff < 4.0 Hz && xdtdelta < 0.1s` → 去重（保留最高 sync）
**JTDX**：`fdiff < 4.0 Hz (SWL=3.0)` → 去重
**ft8rs**：`fdiff < 0.5 Hz && tdiff < 0.04s` → 去重（我们改的）

ft8rs 的去重非常激进（0.5Hz），WSJT-X/JTDX 是 4.0Hz。这意味着我们对近乎同频的信号很宽容（✅），但这是修复 bug 后留下的。

### 2.5 dt 搜索范围

**WSJT-X**：`jz=62`（±2.5s），标准
**JTDX**：`jzb=-62..jzt=62` 标准，SWL 模式 `jzb=-86..jzt=86`（±3.5s）
**ft8rs**：使用了 `jz=62`（与 WSJT-X 一致）

JTDX 的 SWL 模式扩展 dt 范围到 ±3.5s，这可能在非标准时间偏移时多抓到信号。

---

## 三、下调采样（ft8_downsample）差异

### 3.1 Taper 窗口

**JTDX**：
- 使用 `windowc1`（预计算的 cos² taper）
- 边缘增强：`cd0(0)*=1.93, cd0(799)*=1.7, cd0(800)*=1.7, cd0(3199)*=1.93`（高灵敏度模式）
- 低灵敏度模式也做边缘增强：`c1(45)*=1.49, c1(54)*=1.49, c1(3145)*=1.49, c1(3154)*=1.49`
- 时移版本：`c2 = (c0[i] + c0[i+1])/2`，`c3 = (c0[i-1] + c0[i])/2`（用于虚拟 QSO）

**ft8rs**：
- 自定义 cos² taper（TAPER_SIZE=101）
- 无边缘增强
- 无时移版本

**影响**：JTDX 的边缘增益补偿了频带边缘的信号损失。时移版本在虚拟 QSO 检测中至关重要（见下文）。

### 3.2 FFT 缓存策略

**JTDX**：只在 `newdat1=true` 时重新计算长 FFT（cx），之后复用 `cxx`
**ft8rs**：每次调用重新做 FFT（没有缓存机制）

**影响**：性能而非灵敏度。但 JTDX 的缓存允许更多 pass 而不会重复计算 FFT。

---

## 四、多 Pass 策略（架构级差异）

### 4.1 WSJT-X 策略

```
npass=3 (depth=3)
pass 1: lsubtract=true, ndeep=2, syncmin=1.3
pass 2: lsubtract=true, ndeep=3, syncmin=1.3  
pass 3: lsubtract=true, ndeep=3, syncmin=1.3

nagain: dd=iwave, ifa=nfqso-20, ifb=nfqso+20 (窄带)
```

简单直接。3 pass + 窄带重搜。

### 4.2 JTDX 策略（极其复杂）

```
npass=3/6/9 (取决于 nft8cycles)
nft8cycles=1: npass=3
nft8cycles=2: npass=6  ← 默认
nft8cycles=3: npass=9

每个 cycle 3 pass:
  pass 1/4/7: syncmin=1.5 (lowth=1.225), sqrt, newdat1=true
  pass 2/5/8: syncmin=1.5, power, newdat1=true
  pass 3/6/9: syncmin=1.1, abs, lsubtract=false (最后 pass 不减)

在 pass 4: dd8[i]=(dd8[i]+dd8[i+1])/2  ← 数据平滑！
在 pass 7: dd8 恢复原值
```

**关键创新**：
1. **数据平滑**：pass 4 对 dd8 做相邻平均（低通滤波），减小噪声方差
2. **不同谱表示**：每轮用不同的 sync 计算方式
3. **不同 syncmin**：pass 3/6/9 降到 1.1
4. **并行频段分割**：多线程按频率段并行解码

### 4.3 ft8rs 策略

```
MAX_DECODE_PASSES_DEPTH3 = 4
pass 0: syncmin=0.8, power, lsubtract=true
pass 1: syncmin=1.1, power, lsubtract=true
pass 2: syncmin=1.1*1.5, power, lsubtract=true
pass 3: syncmin=1.1*1.5, power, lsubtract=true
```

简单但比 WSJT-X 多一轮（4 vs 3）。

**缺失**：
- 没有数据平滑 pass
- 没有不同谱表示
- nagain 用的是 residual 数据（已减法的），而非原始数据

---

## 五、跨时隙信号复用（JTDX 独有，关键差距）

这是 JTDX 最精巧的设计——**不只是在解码器层面做信号处理，而是在调用层面向下传递历史信息**。

### 5.1 JTDX 信号记忆系统

JTDX 维护跨时间槽的信号记忆：

```
even_copy: 上一偶数时隙（0s/30s）解码的消息 + 频率 + dt
odd_copy:  上一奇数时隙（15s/45s）解码的消息 + 频率 + dt

evencq(40, nthreads):  保存的 CQ 信号（含完整的 cs(0:7,79) 复符号！）
oddcq(40, nthreads)
evenmyc(25, nthreads): 保存的 MyCall 信号
oddmyc(25, nthreads)
evenqso(1, nthreads):  保存的 QSO 信号
oddqso(1, nthreads)

calldteven(150): 过去150个偶数时隙的呼号+dt
calldtodd(150):  过去150个奇数时隙的呼号+dt

lastrxmsg: 上次收到的消息（含 xdt, lastmsg）
```

### 5.2 虚拟 QSO 处理（lvirtual2/lvirtual3）

当 dt > 4.9s 或 dt < -4.9s 时（信号跨时隙边界），JTDX 创建"虚拟 QSO"：
- `lvirtual2`：使用 cd2（时间前移的 downsampled 数据）
- `lvirtual3`：使用 cd3（时间后移的 downsampled 数据）
- 使用已保存的 lastrxmsg 的 xdt 作为初始时间猜测

**ft8rs** 完全没有这个机制。边界信号（dt 接近 ±5s）在 ft8rs 中就是盲区。

### 5.3 信号关联检测

JTDX 的 ft8b 在解码前会检查：
1. 当前候选频率是否与上次解码的某个 CQ 信号匹配（±3Hz, ±0.19s dt）
2. 如果是，则用保存的复符号进行信号关联检测（ft8sd1, ft8mf1, ft8mfcq）
3. 如果信号关联成功，直接输出解码结果，跳过 LDPC

**这意味着 JTDX 可以用比 LDPC 低得多的 SNR 检测到信号**，只要它之前在同一频率/时间出现过。

### 5.4 DX Calls 搜索

JTDX 还维护呼号频率记忆，对已知 DX 呼号的信号做特殊搜索：
- `lenabledxcsearch` / `lwidedxcsearch`
- 特定 tone 模式匹配（idtonecqdxcns, idtonedxcns73 等）
- 对稀有 DX 台站做额外解码尝试

---

## 六、信号减法策略差异

### 6.1 WSJT-X

```
pass 1-3: 解码 → 立即 subtractft8（lsubtract=true）
nagain: 窄带重搜原始数据
a7: 跨时隙重搜历史信号
```

**关键**：减法用 `gen_ft8wave` 生成的 NSPS=1920 全分辨率波形 → 高度精确。

### 6.2 JTDX

```
pass 1-2: 解码 → 立即 subtractft8
pass 3: lsubtract=false（不减，保留残差给弱信号）
多个 cycles 之间：保存已知信号，在后续 pass 中用已减法的数据
特殊：lsubtracted 跟踪哪些频率已减法，避免重复
```

### 6.3 ft8rs

```
pass 0: 并行解码 → 批量 subtractft8
pass 1-3: 同 pass 0
nagain: 在原始数据（dd_original）上窄带重搜
```

**差异**：
- ft8rs 的并行解码+批量减法在 pass 内部，同一 pass 内的候选看的是相同残差
- JTDX 在 pass 3 完全不做减法（`lsubtract=false`），这保留了最干净的残差给最弱信号

---

## 七、AP 解码差异

### 7.1 WSJT-X AP 解码

深度集成到 ft8b 内部：
- `iaptype 1-6`：CQ, MyCall, MyCall+DxCall, MyCall+DxCall+RRR/73/RR73
- 根据 `nQSOProgress`（0-5）自动选择当前应该解码的 AP 类型
- `naptypes(0:5, 1:4)` 表：每个 QSO 状态对应优先尝试的 AP 类型
- AP 成功解码后降低 `nharderrors` 阈值要求
- `lapon` 标志开启 AP

### 7.2 JTDX AP 解码

**比 WSJT-X 更全面的 AP 系统**：

1. **标准 AP**（同 WSJT-X iaptype 1-6）
2. **CQ 信号 AP**：利用保存的 CQ 信号（evencq/oddcq 中的完整复符号）做匹配滤波
3. **MyCall 信号 AP**：利用保存的 MyCall 信号
4. **DX Calls AP**：`apsymdxns1`, `apsymdxnsrrr`, `apsymdxnsrr73`, `apsymdxns73` 等
5. **非标准呼号 AP**：`apsymmyns1`, `apsymmyns2`, `apsymmynsrr73` 等
6. **Fox/Hound AP**：特殊竞赛模式
7. **频谱报告 AP**：`lfoxspecrpt`, `lfoxstdr73`

### 7.3 ft8rs AP

只有框架（Pass 9/10），需要显式传入 `mycall: Some(...)` / `hiscall: Some(...)` 才启用。

**缺失**：
- 没有自动 AP 类型选择（nQSOProgress → iaptype 映射）
- 没有 contest 模式 AP
- 没有任何跨时隙 AP 信号关联

---

## 八、SNR 计算差异

### 8.1 WSJT-X SNR

```fortran
! 信号功率 / 噪声功率
xsig = Σ s8(itone(i), i)²  // 79个符号的信号功率
xnoi = Σ s8(mod(itone(i)+4,7), i)²  // 对应位置的噪声功率

! sbase 归一化版本
xsnr2 = 10*log10(xsig/xbase/3e6 - 1.0) - 27.0

! 直接信噪比版本
xsnr = 10*log10(xsig/xnoi - 1.0) - 27.0

! nagain 时用直接版
if (.not.nagain) xsnr = xsnr2
```

两种 SNR，在不同场景用不同的。xsnr2 考虑了频谱底噪（sbase）。

### 8.2 JTDX SNR

同样两种 SNR 计算，但额外有：
- sync SNR：`synclev/snoiselev`（在 ft8b.f90 中计算）
- 多层 SNR 检查（见 sync 质量检查部分）

### 8.3 ft8rs SNR

```rust
let arg = xsig / xnoi.max(1e-30) - 1.0;
let snr = 10.0 * arg.log10() - 27.0;
if snr < -24.0 { -24.0 } else { snr }
```

只有直接信噪比版本，**缺少 sbase 归一化的 SNR**。

**影响**：sbase 归一化的 SNR 对识别"在噪声基底以上"的信号更准确。在干净频段，直接 SNR 可能低估信号。在嘈杂频段，可能高估。

---

## 九、其他 JTDX 独有技巧

### 9.1 LDPC 前信号预筛选

JTDX 的 ft8b.f90 在进入 LDPC 之前有多层预筛选：
- **信号功率检测**：`s82 = sqrt(s8)` → 用于 ft8s 检测
- **Costas SNR 估计**：计算 sync 符号的平均 SNR（synclev/snoiselev）
- **CQ 信号评分**：`rscq` (reliable score for CQ)
- **MyCall 信号评分**：`nmic`
- **QSO 结束信号评分**：`nqsoend` (73/RR73/RRR)

如果某一层的评分不够，就不进入 LDPC，节省计算并减少误检。

### 9.2 信号反转处理

JTDX 做 **信号反转处理**（`lreverse`）：将接收信号时间反转（csymbr = conj(flip(csymb))），然后同时处理正反两个方向。这利用了 FT8 的对称性提供额外的分集增益。在 pass 2/5/7 等更深层 pass 使用。

### 9.3 符号电平均衡

JTDX 对弱信号做符号电平校正：
```fortran
if(syncav < 2.5) then
    csymb(1) *= 1.9; csymb(32) *= 1.9  ! 两端符号增强
    scr = sqrt(|csymb(1)|) / sqrt(|csymb(32)|)  ! 平衡因子
    if(scr > 1.0) csymb(32) *= scr
    else csymb(1) /= scr
endif
```

以及音调功率均衡：
```fortran
! 对过强的音调做衰减，补偿频率选择性衰落
if(spr > 1.5) then
    s8(kb,:) /= spr; cs(kb,:) /= sqrt(spr)
endif
```

**ft8rs 完全没有这两步**。

### 9.4 多线程频段分割

JTDX 的并行策略是**按频率段分割**（2/4/6/8/...线程各负责一段频率），而不是候选级别并行。这保证了：
1. 同一频率段内的信号串行处理（减法顺序正确）
2. 不同频率段完全独立（无竞争）
3. 跨线程信号记忆通过共享数组传递

### 9.5 AGC 补偿（lagcc）

JTDX 有可选的 AGC 补偿模式（`lagcc`），调用 `agccft8()` 预处理数据，减小接收机 AGC 对弱信号的影响。

---

## 十、调用层面的差异——不仅仅是解码器

这是关键洞察：**WSJT-X 和 JTDX 的解碼能力很大一部分来自调用层面的编排，而非纯粹的解码器内部算法**。

### 10.1 时间槽对齐

在实际接收中，WSJT-X/JTDX 精确知道当前 UTC 时间槽（0/15/30/45s），因此：
- 知道应该收哪个方向的信号（even/odd）
- 可以把过去时隙的解码结果带入当前时隙
- 可以利用相邻时隙的信号重叠（virtual QSO）

**ft8rs 的长解码测试**没有使用时隙信息——只是把整个录音按 15s 分段解码。这丢失了跨时隙的上下文。

### 10.2 减法顺序

在实时接收中，WSJT-X/JTDX 可以做到：
1. 解码强信号 → 减法
2. 再用更敏感的设置在残差上重搜
3. 利用上一步解码结果的频率/dt 缩小搜索范围（nagain）

ft8rs 的批量减法虽然正确，但同一 pass 内的候选看的是相同残差。如果两个信号在频率上靠近，强信号可能掩蔽弱信号。

### 10.3 已知信息的利用

在实时操作中：
- **mycall**：操作者自己的呼号 → 用来做 AP 解码
- **hiscall**：正在通联的对方呼号 → 用来做 AP 解码
- **nQSOProgress**：QSO 进行到哪一步 → 决定优先尝试哪种消息类型
- **nftx**：自己发射的频率 → 期望回复在此附近
- **nfqso**：QSO 频率 → 该频率附近的信号优先级更高

**ft8rs 的长解码测试**只知道一段录音，没有任何上下文。

### 10.4 渐进式解码（nzhsym）

WSJT-X/JTDX 支持在不同处理阶段使用不同数量的符号：
- `nzhsym=41`：只处理前 41 个符号（早期解码，用于快速显示）
- `nzhsym=47`：前 47 个符号（中间阶段）
- `nzhsym=50`：全部 50 个信息符号（最终阶段）

这允许早期解码结果用于后续阶段的减法，形成**层层推进**的解码策略。

### 10.5 nft8cycles 多轮解码

JTDX 支持 1/2/3 轮完整解码循环（`nft8cycles`）。每轮用不同的参数（sync 计算方式、syncmin、频谱平滑），相当于把同一段数据用不同的"视角"看多次。

---

## 十一、灵敏度提升建议（按优先级）

### 高优先级（预计 +30-50 条）

1. **增强同步质量门控**
   - 从简单 nsync 计数改为 SNR-based 评分（参考 JTDX nsyncscore/scoreratio）
   - 移除 `nsync ≤ 3 → bail` 的硬门控，改为基于 SNR 质量的软门控
   - 预期：低 nsync 但高 SNR 的信号可以得到解码机会

2. **实现 3 种 sync 计算方式**
   - pass 0-1: sqrt 幅度谱（对弱信号友好）
   - pass 2-3: power 功率谱
   - nagain: abs 绝对值谱
   - 预期：不同谱表示覆盖不同类型的弱信号

3. **实现数据平滑 pass**
   - 在多 pass 之间对时域数据做相邻平均 `dd[i] = (dd[i] + dd[i+1]) / 2`
   - 然后用平滑后的数据重新 sync8
   - 预期：降低噪声方差，提升低 SNR 信号检测

4. **修复 sbase 访问索引**
   - sync8 用 4096-point FFT (df=2.93)，但 ft8b 用 DOWNSAMPLE_DF (0.0625) 索引
   - 需要统一索引方式或存储正确分辨率的 sbase
   - 预期：使 sbase 归一化对 >128Hz 信号生效

### 中优先级（预计 +15-25 条）

5. **实现跨段信号记忆**
   - 保存每段解码结果的频率+dt+msg
   - 下一段解码时利用这些信息：
     - 同频率/时间的信号关联检测（跳过 LDPC 直接用匹配滤波）
     - 边界信号虚拟 QSO 处理（cd2/cd3）
   - 预期：利用时间连续性提高弱信号检出率

6. **端极信号处理**
   - dt > 4.5s 或 dt < -4.5s 的信号做特殊的时移 downsampling
   - 参考 JTDX 的 lvirtual2/lvirtual3 + cd2/cd3
   - 预期：减少边界信号丢失

7. **不同 pass 使用不同 syncmin**
   - 前几 pass 用较高 syncmin（减少误检）
   - 后几 pass 用较低 syncmin（允许更弱的候选进入）
   - 参考 JTDX: pass 1=1.5→1.225, pass 3=1.1

8. **nagain 改用原始数据**
   - 当前 nagain 在所有 pass 的 residual 上做
   - 改为使用原始 dd_original（减去了已知信号之后）
   - 参考 WSJT-X 的 nagain 模式：`dd=iwave`

### 低优先级（预计 +5-10 条）

9. **Taper 窗口增强**
   - JTDX 的 cos² 窗口 + 边缘增益
   - 实现高灵敏度模式（lhighsens 风格的边缘放大）

10. **完善的 AP 解码**
    - 从 HashCallBook 自动获取潜在 hiscall
    - 实现 nQSOProgress → iaptype 自动映射
    - 在测试中提供 mycall/hiscall 参数

11. **符号电平均衡**
    - 弱信号时对 Cos² taper 边缘符号增强
    - 对频率选择性衰落的音调做功率均衡

---

## 十二、结论

ft8rs 与 JTDX/WSJT-X 的灵敏度差距 **主要不在解码器核心算法（LDPC、subtractft8、sync8d）**，而在以下几个层面：

1. **信号检测门控**（最大差距）：JTDX 的 SNR-based 多层门控比 ft8rs 的 nsync 简单门控精细得多
2. **多视角重搜**：3 种 sync 计算方式 + 数据平滑 = 多角度扫描同一数据
3. **跨时隙记忆**：信号不会孤立存在——JTDX 利用历史信息大幅提升检测能力
4. **调用层编排**：多 pass、多 cycle、不同参数的组合策略比解码器内部算法更重要

**最有效的 3 个改进**（ROI 最高）：
1. 实现 JTDX 风格的 SNR-based 同步质量门控
2. 添加数据平滑 pass
3. 实现 sqrt/abs 两种额外的 sync 计算方式
