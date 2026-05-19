# WSJT-X 源码深度对比分析

## 目标
20/20 ≤30s, 420/449 长解码

## 当前状态
20/20: 50s, 长解码: 362/449 (80.6%), 446s

## 已验证无增益的改动
| 尝试 | 结果 | 耗时变化 |
|---|---|---|
| FFT 3840 | 20/20 失败 | - |
| nagain syncmin 1.5→1.15 | 无增益 | - |
| nagain syncmin 1.5→1.0 | 更慢 | + |
| 平滑数据 | 零增益 | + |
| 跨段记忆窄带搜索 | 零增益 | ++ |
| AP 盲解 | 零增益 | ++ |
| AbsSum 第 3 轮 | +4/+252s | +++ |
| 3840 FFT | 19/20 | ++ |
| xbase LLR 归一化 | 零增益 | +++ |
| maxosd 5→2 | 零增益 | - |
| syncmin pass 缩放 | 零增益，反而更慢 | + |

## 核心差异（可能还有空间）

### 1. nagain 范围
WSJT-X: nagain 只搜 nfqso±20Hz（用户双击的频率）
ft8rs: 搜所有已解码频率的 ±20Hz（更宽，更慢）

### 2. 减法精炼 (lrefinedt)
WSJT-X: depth≥2 时，nzhsym=47 用 lrefinedt 精炼 dt，再减法
ft8rs: 无

### 3. 进度式解码
WSJT-X: nzhsym=41→47→50 分阶段处理
ft8rs: 每次都完整解码

### 4. SNR 公式
WSJT-X: xsnr = xsig/xnoi - 1, xsnr2 = xsig/xbase/3e6 - 1
       if (!nagain) xsnr = xsnr2
ft8rs: 只用 xsig/xnoi - 1

### 5. 最终假信号过滤
WSJT-X: nsync≤10 && xsnr<-24 → 标记为假信号
ft8rs: 无此过滤

### 6. 候选排序优先级
WSJT-X: nfqso±10Hz 候选置顶，再按 sync 降序
ft8rs: 只按 sync 降序

### 7. 数据窗口
WSJT-X: 180000 samples (15s@12kHz), 零填充到 3840 FFT
ft8rs: 同

## 结论
经过大量尝试，当前 362/449 已是 sync8 + ft8b 框架下的极限。
要追上 WSJT-X 的 420+，需要架构级改动：
1. 渐进式解码（nzhsym 分阶段）
2. lrefinedt dt 精炼
3. 更精确的 SNR/xbase 校准
4. 跨段信号关联（JTDX 风格）
5. AP 解码（已知 mycall/hiscall）

但这些改动都需要真实 QSO 上下文，盲解码场景下收益有限。
