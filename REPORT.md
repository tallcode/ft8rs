# FT8 解码技术分析报告

> 项目：ft8rs vs WSJT-X FT8 解码器技术差异分析与改进方案
>
> 最终成果：20/20 消息，11.4s（3× 加速），零编译警告

### ✅ 里程碑：20/20 达成！（2026-05-17）

| 指标 | 初始 | 最终 |
|------|------|------|
| 解码数 | 16/20 (80%) | **20/20 (100%)** ✅ |
| 时间 | ~7s | **~11.4s** |
| 编译警告 | 多个 | **0** ✅ |

成功解码的 4 条弱信号：
- **KD2UGC F6GCP R-23** @472Hz, -12dB SNR
- **K1BZM EA3CJ JN01** @2522Hz, -12dB SNR
- **CQ EA2BFM IN83** @2280Hz, -16dB SNR
- **WA2FZW DL5AXX RR73** @2546Hz, -19dB SNR

## 性能优化历程

| 版本 | 耗时 | 解码数 | 技术 |
|------|------|--------|------|
| v0 基线 | 7.0s | 16/20 | 无信号减法 |
| v1 subtractft8 | 33.8s | 20/20 | 时域 O(N×M) 循环卷积 |
| v2 去除取模 | 27.9s | 20/20 | 预扩展数组避免 rem_euclid |
| v3 rayon 并行 | 13.4s | 20/20 | 4 核并行化卷积循环 |
| **v4 FFT 卷积** | **11.4s** | **20/20** | O(N·logN) 线性卷积 |

---

## 🔬 subtractft8 突破经验

### subtractft8 的 3 个隐藏 Bug

通过逐行对比 Fortran `subtractft8.f90`，发现并修复了 3 个 bug：

| Bug | Fortran | ft8rs 原实现 | 后果 |
|-----|---------|-------------|------|
| FFT 尺寸 | NFFT=180,000 | 零填充到 262,144 | LPF 窗口位置偏移 |
| IFFT 归一化 | four2a 不归一化 | 1/N 归一化 | LPF 增益计算错误 |
| cshift 方向 | 左移 (cshift,+) | rotate_right | 窗口再次偏移 |

### 最终方案：FFT 线性卷积（v4）

**核心洞察**: Fortran 的循环卷积在 NMAX=180,000 内进行，signal 之外是零填充 → 等价于线性卷积。

**实现**:
```rust
// 1. camp 扩展 NFILT 样本环形 halo（确保 FFT 线性卷积 = 时域循环卷积）
ext[j] = camp[(j - HALF_FILT) mod NFRAME]  // j = 0..NFRAME+NFILT

// 2. FFT 卷积
cfilt = IFFT(FFT(ext) × FFT(window))  // window FFT 预计算，OnceLock 缓存

// 3. 提取结果
cfilt[i] = result[i + NFILT]  // 移位对齐
```

**为什么之前 FFT 失败**:
1. Halo 只用了 HALF_FILT=2000，但卷积需要 NFILT=4000 样本的边界覆盖
2. 输出偏移用了 HALF_FILT 而非 NFILT
3. 修复这两点后 FFT 版本精度完全对齐时域版本

### 关键技术洞察

| 洞察 | 说明 |
|------|------|
| **减法精度是灵敏度核心** | 不精确的减法→残差掩蔽弱信号 |
| **逐行对齐 Fortran** | 任何微小的偏移/缩放/方向差异都会破坏结果 |
| **gen_ft8wave 用 NSPS=1920** | 波形生成全分辨率，不是检测的 48 samples/symbol |
| **FFT 尺寸无需匹配 Fortran** | 线性卷积 + 正确 halo = 与 Fortran 循环卷积等价 |
| **先保证正确，再优化** | 时域版本先上线验证 → FFT 替换时回退到已知正确版本 |

---

### 源码验证

本地 WSJT-X 源码位于 `wsjtx/` 目录，版本与 saitohirga/WSJT-X GitHub 镜像一致。

已验证的关键文件（`wsjtx/lib/ft8/` 下）：

| 文件 | 行数 | 功能 |
|------|------|------|
| `ft8_decode.f90` | 297 | 主解码入口，多pass策略 |
| `ft8b.f90` | 516 | 单信号精细解码 + AP解码 |
| `sync8.f90` | 147 | 粗同步搜索（2D相关网格） |
| `sync8d.f90` | 39 | Costas精细同步（复相关） |
| `subtractft8.f90` | 117 | 信号减法（复基带+LPF+精炼） |
| `ft8_downsample.f90` | 42 | 宽频提取+下采样 |
| `ft8c.f90` | ~100+ | a7模式信号重检 |
| `ft8apset.f90` | — | AP符号表初始化 |
| `ft8_a7.f90` | — | a7历史数据管理 |

**确认：本地源码与网上获取的参考代码一致，以下分析基于本地源码。**

---

## 测试基线与质量门控

| 测试文件 | 结果 | 状态 |
|----------|------|------|
| 210703_133430.wav | 20/20, 11.4s | ✅ |
| `test_20_message_baseline` | 断言 ≥20 条 | ✅ CI 阻断 |

**当前架构**: Pass 1 → subtract → Pass 2 → subtract → Pass 3 → nagain 窄带重搜

## 仍有优化空间（不妥协精度）

| 方向 | 预期收益 | 难度 |
|------|---------|------|
| gen_ft8wave GFSK pulse 缓存 | ~2s | 低 |
| BP 自适应迭代 | ~1s | 中 |
| sync8 候选去重优化 | ~0.5s | 低 |

---

## 参考源码

已验证的 Fortran 源文件 (`wsjtx/lib/ft8/`)：

| 文件 | 功能 |
|------|------|
| `subtractft8.f90` | 信号减法（复基带+LPF） — 最关键对齐文件 |
| `gen_ft8wave.f90` | GFSK 波形生成 (NSPS=1920) |
| `ft8b.f90` | 单信号精细解码 + AP 解码 |
| `sync8.f90` / `sync8d.f90` | 粗同步搜索 / Costas 精细同步 |
| `ft8_downsample.f90` | 宽频提取+下采样 |

## 参考项目

- **ft8ts** (`ft8ts/src/index.ts`) — TypeScript 参考端口 (16/20)
- **wsjtx_lib** (`wsjtx_lib/`) — C++ 封装 Fortran（交叉验证）

---

## 附录：WSJT-X 差异分析摘要

### 已对齐的部分
- **sync8/sync8d**: FFT 尺寸、Costas 数组、相关搜索 → 完全一致
- **ft8_downsample**: 频带提取、cos² taper → 完全一致
- **ft8b 核心**: 时偏/频偏搜索、软符号提取、bit metrics → 完全一致
- **LDPC**: normalizebmet、scalefac=2.83、BP + OSD → 完全一致
- **subtractft8**: 复基带 LPF 信号减法 → 完全一致（本次突破）

### 未实现（可能提升灵敏度）

| 特性 | 说明 | 预期收益 |
|------|------|----------|
| **AP 解码** | iaptype 1-6 已知比特注入 LLR | 2-4dB |
| **a7 历史复用** | 跨时隙信号记忆 (ft8c.f90) | 1-3 条 |
| **lrefinedt** | ±90 样本时偏精炼减法 | 残差更小 |
| **Contest 模式** | 8 种 contest 特定比特模式 | 场景相关 |
| **渐进式解码** | nzhsym 41→47→50 分段处理 | 实时优势 |

### 当前 AP 代码状态

`try_decode_passes` 中有 4 个 AP 解码 pass（CQ 模式 + i3/n3 约束），但**实测对 20/20 基线无贡献**（禁用后结果不变）。真正的 WSJT-X AP 需要已知呼号信息（apsym），当前未实现。

---

