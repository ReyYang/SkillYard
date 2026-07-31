# SkillYard 集成 Qwen 的最低硬件要求

> 调研日期：2026-07-28
>
> 状态：历史条件评估。SkillYard 1.1.0 已确认不集成本地 Qwen 或其他本地 LLM，本文只说明“如果未来重新选择 Qwen”时的硬件推导，不能作为当前系统要求或实施计划；当前产品边界见 [1.1.0 Agent 产品规格](../prd/0002-skillyard-1.1.0-agent.md)。
>
> 评估对象：`Qwen2.5-0.5B-Instruct-GGUF` 的官方 `Q4_K_M` 权重
>
> 运行假设：SkillYard 内嵌原生 `llama.cpp`，使用 Apple Silicon 的 Metal／Accelerate 路径；不要求用户安装 Ollama、Python 或独立 localhost Server

## 结论

如果 SkillYard 使用 **Qwen2.5-0.5B-Instruct `Q4_K_M`**，现有平台下限不需要提高：

| 项目 | 最低支持配置 | 推荐配置 |
| --- | --- | --- |
| 系统 | macOS 14 Sonoma | macOS 14 或更高版本 |
| 处理器 | Apple M1 或更新的 Apple Silicon | 任意 M1／M2／M3／M4 或后续 Apple Silicon |
| 统一内存 | **8 GB** | **16 GB** |
| 空闲磁盘 | **2 GB** | **2 GB 以上** |
| 独立显卡 | 不需要 | 不需要 |
| Neural Engine | 不作为运行前提 | 不作为运行前提 |

因此，产品可以继续写成：

> macOS 14+、Apple Silicon、8 GB 统一内存；16 GB 推荐。

但这里的“8 GB 可支持”是由模型体积、KV cache 和运行结构推导出的**工程下限**，还不是实测结论。正式把它写入 Release 支持承诺前，必须在 **M1／8 GB** 设备上测量完整 SkillYard 进程的峰值内存、首字延迟和连续处理表现。仅在开发机或更高配 Mac 上运行成功，不能证明最低配置。

## 为什么 M1／8 GB 足够成为最低候选

### 1. 权重只有 491 MB

Qwen 官方 [GGUF 文件列表](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/tree/main) 给出的 `qwen2.5-0.5b-instruct-q4_k_m.gguf` 文件体积是 **491 MB**。这是磁盘文件体积，也是常驻模型内存的主要量级，但不能直接等同于完整应用的运行内存。

官方 [模型卡](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF) 还确认：

- 参数量是 0.49B；
- 24 层；
- 14 个 Query heads、2 个 KV heads；
- 模型自身支持 32,768 token 上下文；
- 官方 `llama.cpp` 示例直接使用 `Q4_K_M`；
- 许可证是 Apache-2.0。

### 2. SkillYard 不需要使用完整 32K 上下文

Skill 描述、摘要和分类属于短文本任务。产品运行参数应固定为：

- context：2,048 或 4,096 token；
- 单次输出：不超过 256 token；
- 同一时刻只执行一个推理请求。

不应因为模型声称支持 32K，就为每次短描述任务分配完整 32K 上下文。

Qwen 官方 [`config.json`](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct/blob/main/config.json) 给出：

- `hidden_size = 896`；
- `num_attention_heads = 14`；
- `num_key_value_heads = 2`；
- `num_hidden_layers = 24`。

因此每个 attention head 的维度是 `896 / 14 = 64`。按 `llama.cpp` 默认的 F16 K/V cache 计算：

```text
每 token KV bytes
= 层数 × KV heads × head dimension × K/V 两份 × F16 2 bytes
= 24 × 2 × 64 × 2 × 2
= 12,288 bytes
```

对应的理论 KV cache 是：

| Context | 理论 F16 KV cache |
| ---: | ---: |
| 1,024 token | 12 MiB |
| 2,048 token | 24 MiB |
| 4,096 token | 48 MiB |
| 8,192 token | 96 MiB |
| 32,768 token | 384 MiB |

该计算来自模型结构与缓存数据类型，不包括 tensor 对齐、元数据和计算 buffer。[`llama.cpp` KV cache 源码](https://github.com/ggml-org/llama.cpp/blob/master/src/llama-kv-cache.cpp) 显示缓存按每层的 K/V embedding 维度与 context cells 分配；官方参数说明中 K/V cache 默认类型是 F16。

对于 SkillYard 建议的 4K context，已知的主要常驻项只有约：

- GGUF 权重：491 MB；
- F16 KV cache：约 48 MiB；
- 另加 `llama.cpp`、Metal、tokenizer、计算图 buffer、Tauri WebView 和 SkillYard 自身状态。

后面这些 buffer 会随 `llama.cpp` 版本、batch、Metal backend 和集成方式变化，当前没有实测值。不能把“491 MB 模型”描述成“只占 491 MB 内存”，也不应在未测量前承诺一个精确 RSS。合理的验收预算是：**启用模型后的 SkillYard 额外峰值内存不得超过 1.5 GB**。这不是当前实测结果，而是为了保证 8 GB 设备仍有系统余量设置的产品门槛。

### 3. Apple Silicon 不需要额外显存

Apple 官方说明，Apple Silicon 的 GPU 与 CPU 使用统一内存；Metal 的 [`hasUnifiedMemory`](https://developer.apple.com/documentation/metal/mtldevice/hasunifiedmemory) 表示 GPU 与 CPU 共享内存。模型权重、GPU buffer 和应用内存因此都从同一份 8 GB 中取得，不存在“另需 1 GB 独立显存”的要求。

这也意味着不能把 8 GB 统一内存理解为“8 GB 系统内存外加 GPU 显存”。当浏览器、IDE、Agent 应用同时占用大量内存时，macOS 可能发生压缩或 swap，模型仍可能运行，但延迟会升高。

Apple 官方 [M1 MacBook Air 规格](https://support.apple.com/en-ie/111883) 确认基础机型具有：

- Apple M1；
- 8 核 CPU；
- 7 核或 8 核 GPU；
- 8 GB 统一内存；
- 256 GB SSD。

Apple 的 [macOS Sonoma 兼容列表](https://support.apple.com/en-gb/105113) 包含 2020 年 M1 MacBook Air。它同时满足 SkillYard 当前的 macOS 14、Apple Silicon 边界，所以 M1／8 GB 是一个自然、可验证的最低设备，而不是为了 Qwen 新造出的硬件分层。

### 4. `llama.cpp` 已明确支持 Apple Silicon

[`llama.cpp`](https://github.com/ggml-org/llama.cpp) 官方说明把 Apple Silicon 视为一等平台，并通过 ARM NEON、Accelerate 和 Metal 优化。Qwen 官方 GGUF 模型卡也直接给出 `llama.cpp` 的 macOS 使用方式。

这条路线使用 CPU 与 Metal GPU，不以 Apple Neural Engine 为运行前提。因此硬件要求不应写成“需要 Neural Engine”，也不需要为不同 Neural Engine 核数建立兼容表。

## 最低配置下必须限制的运行方式

8 GB 下限成立的前提不是“任何运行参数都可以”，而是 SkillYard 保持短任务、低并发：

1. **按需加载模型。** 扫描、安装、挂载和更新不能启动模型；只有需要翻译、摘要或分类时才加载。
2. **串行推理。** 一个 Skill 完成后再处理下一个，不开多个并行 slot。
3. **限制 context。** 默认 2K，确有必要时最多 4K；不能直接采用模型的完整 32K。
4. **限制输出。** 翻译、摘要和分类合计不超过 256 token。
5. **允许释放。** 模型长时间未使用后可以卸载，避免 SkillYard 在后台永久占用约 0.5–1.5 GB。
6. **不启动通用模型服务。** 应由应用进程通过窄接口调用内嵌 runtime，避免额外的 Ollama／Python／HTTP Server 进程和不可控默认参数。
7. **模型失败不影响生命周期功能。** 内存不足、模型损坏或推理失败时，只回退到作者原文，不能阻断扫描、接管、安装、挂载或更新。

这些限制不是为了支持极端环境，而是让模型能力符合 SkillYard 的短文本用途，并保住现有最低设备。

## 磁盘为什么建议预留 2 GB

491 MB 只是最终 GGUF。实际安装或更新还可能同时存在：

- 正在下载的临时文件；
- 校验完成后的正式模型；
- 原生 `llama.cpp` runtime 和 Metal shaders；
- 应用本体；
- 模型替换期间的新旧文件。

如果模型采用首次使用时下载，最简单的安全替换过程至少可能同时持有一份临时文件和一份正式文件，已经接近 1 GB。预留 **2 GB** 可以覆盖模型、runtime、临时下载和替换余量。

这不代表 SkillYard 永久占用 2 GB。最终占用必须由真实 `.app` 和真实模型交付方式测量。若把模型直接放进 Release，不含模型的下载包会增加接近 491 MB；GGUF 已量化，不能预设 ZIP 会把它大幅压缩。

## 模型尺寸变化会怎样改变硬件要求

Qwen 是一个模型家族。“使用 Qwen”本身无法确定硬件要求。官方 Qwen2.5 `Q4_K_M` 文件体积是：

| 模型 | 官方 Q4_K_M 权重 | 硬件规划判断 |
| --- | ---: | --- |
| 0.5B | 491 MB | M1／8 GB 可作为最低候选；16 GB 推荐 |
| 1.5B | 1.12 GB | 8 GB 理论上仍能容纳；16 GB 更适合正式支持 |
| 3B | 2.10 GB | 不建议继续对 8 GB 承诺稳定体验；16 GB 起 |
| 7B | 4.68 GB（两个分片合计） | 不适合 8 GB；16 GB 是运行下限候选，24 GB 更稳妥 |

来源：

- [Qwen2.5-0.5B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/tree/main)
- [Qwen2.5-1.5B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/tree/main)
- [Qwen2.5-3B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/tree/main)
- [Qwen2.5-7B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-7B-Instruct-GGUF/tree/main)

表中的后两列是基于权重体积和系统余量的规划判断，不是上述型号在 SkillYard 中的实测支持结论。不同模型的层数、KV heads 和 context 配置不同，不能只按参数量等比例推算完整内存。

如果产品希望保留当前“所有受支持设备都能使用全部功能”的简单承诺，**0.5B 是唯一不需要提高 8 GB 下限的合理起点**。一旦升级到 3B 或 7B，就应把模型变成按硬件选择的可选能力，或正式提高最低内存要求。

## 正式承诺前必须完成的验收

最低兼容性需要以下实测证据：

1. 在 M1／8 GB、macOS 14 上启动正式构建，确认 native `arm64` runtime 与 Metal 初始化成功。
2. 固定 2K 与 4K context，分别记录：
   - 模型冷加载时间；
   - 首 token 延迟；
   - 生成速度；
   - SkillYard 完整进程的 idle、推理中和峰值内存；
   - 推理结束／卸载后的内存回收。
3. 连续处理至少 50 个真实 Skill 描述，确认 UI 不冻结、不会创建并行模型实例，也不会因内存压力影响 Mount 等核心操作。
4. 同时开启一个受支持 Agent 应用和普通浏览器，验证 8 GB 的日常场景，而不只测试空白系统。
5. 在模型不可加载或系统内存不足时，确认 Skill 详情仍显示作者原文，主流程继续可用。

VM 可以验证安装、模型文件和功能回退；只有当 VM 确实暴露与正式用户相同的 Metal 推理路径时，它的性能数据才可用于最低硬件承诺。否则仍需要真实 M1／8 GB 设备。

## 最终判断

- **最低兼容目标**：M1、8 GB 统一内存、macOS 14、2 GB 空闲磁盘。
- **推荐配置**：任意 Apple Silicon、16 GB 统一内存、2 GB 以上空闲磁盘。
- **无需新增要求**：独立 GPU、Neural Engine 型号、M2 或更高芯片。
- **最重要的运行约束**：`Q4_K_M`、2K／4K context、单请求、按需加载。
- **尚未确认**：M1／8 GB 的真实峰值 RSS、延迟、热量与翻译／分类质量。

对 0.5B 模型来说，硬件并不是最主要风险。真正需要通过 PoC 决定的是：它能否稳定输出不篡改技术术语的中文翻译、短摘要和固定分类。如果质量不达标，升级到更大 Qwen 会直接改变 SkillYard 的内存、下载体积和最低配置，不能只当作替换一个模型文件。
