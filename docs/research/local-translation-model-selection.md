# Issue #43 本地翻译模型最小可用选型

> 调研日期：2026-07-28
>
> 状态：历史技术调研。SkillYard 1.1.0 已确认采用 OpenAI、智谱 GLM、DeepSeek 的 API-first BYOK 方案，不捆绑、下载或运行本地模型。本文保留本地翻译模型的实测事实，不再作为 1.1.0 实施建议；当前产品边界见 [1.1.0 Agent 产品规格](../prd/0002-skillyard-1.1.0-agent.md)。
>
> 目标环境：macOS 14、Apple Silicon、Tauri 2
>
> 目标任务：把 Skill 作者提供的英文 `description` 离线翻译为简体中文；同时判断同一模型是否适合继续承担分类和摘要。

## 结论

### 最小能跑

**Mozilla Bergamot 当前发布的 `en→zh base-memory` 是最小的可信候选。**

- 四个模型文件的实际下载总量是 **36,745,493 bytes（36.75 MB / 35.04 MiB）**。
- 解压后的四个模型文件是 **49,913,927 bytes（49.91 MB / 47.60 MiB）**。
- 官方 npm WASM runtime `@browsermt/bergamot-translator@0.4.9` 的解包体积是 **5,314,609 bytes（5.31 MB / 5.07 MiB）**。
- 因而模型加 runtime 的最小安装量约为 **55.23 MB / 52.67 MiB**，尚未计入应用打包、缓存元数据和压缩差异。
- 它只做机器翻译，**不能承担 Skill 分类或生成式摘要**。

本次还在本机 Apple Silicon 上用 Node 成功运行了官方 WASM runtime 和当前四个 Release 文件，因此“能跑”已有真实推理证据；**但它不表示模型已经在 Tauri 的 WKWebView 中通过验收**。WKWebView 初始化、Web Worker、CSP、峰值内存以及是否稳定输出简体中文仍需实机测量。

### 最小值得发布

**如果模型只负责翻译已有 `description`，最小值得发布的候选仍然是 Bergamot `en→zh base-memory`，但它目前还不是可直接发布的结论。**

发布前有两个阻塞项：

1. Mozilla 当前模型 registry 没有给每个模型权重提供明确的 `license` 字段。`mozilla/translations` 和 Bergamot 代码仓库使用 MPL-2.0，**不能据此自动推出 GCS 上模型权重也可按 MPL-2.0 再分发**。在得到模型权重的明确许可或权利声明前，不能把权重随 `.app` 捆绑，也不能把“改为首次使用时下载”当作许可问题已经解决。
2. 官方 WASM 文档验证的是 Chrome、Firefox 和 Safari，不是 Tauri 2 的 macOS WKWebView。必须先完成最低支持设备上的验收。

所以更准确的发布判断是：

- **最小发布候选**：Bergamot `en→zh base-memory`。
- **当前已证明可发布的本地模型**：没有；许可与 Tauri 实测证据仍缺失。

### 需要同时分类／摘要时的最小通用 LLM

**Qwen2.5-0.5B-Instruct 的官方 `Q4_K_M` GGUF（491 MB）是目前最小的合理原型候选。**

- 官方还有更小的 `Q2_K`（415 MB）和 `Q4_0`（429 MB），但官方 quickstart 选择 `Q4_K_M`；没有一手证据证明更低量化在 Skill 翻译、分类和摘要任务上仍可靠。
- Qwen2.5 官方说明覆盖中文、英文等 29 种以上语言，并改善了 instruction following 与 JSON 等结构化输出。
- 它可以尝试一次返回“中文翻译 + 固定分类 + 短摘要”，但官方模型卡没有给出 0.5B 模型在本任务上的翻译或分类准确率。
- 因此 **491 MB 是最小的评测起点，不是已证明的发布下限**。

如果 Issue #43 只需要让用户看懂作者已经写好的 `description`，引入约 491 MB 权重和通用生成 runtime 不划算。只有当产品明确要求同一个本地模型生成分类和新摘要时，才应进入 Qwen 路线。

## 产品边界

[Issue #43](https://github.com/ReyYang/SkillYard/issues/43) 同时提出“概要、分类、使用时机”。自动翻译只能解决其中“看懂已有英文描述”的一部分，不能替代分类，也不能从缺失信息中可靠推导使用时机。

当前 [1.0 Product Contract](../1.0-product-contract.md) 明确规定：

- 1.0 不捆绑或下载本地 LLM，也不调用云端模型；
- “解释 Skill”只能在后续版本作为可选能力重新讨论；
- 生产应用不启动 localhost Server，也不捆绑 Python runtime 或 Python sidecar。

因此本调研是后续可选能力的技术输入，**不是 Issue #43 可以直接进入当前 1.0 实现的依据**。若要实施，必须先明确修改产品版本／契约；通用 LLM 的 localhost server 和 Python sidecar 路线也不符合现有运行边界。

## 候选对比

| 路线 | 最小可信权重／模型下载 | runtime 与 macOS 路径 | 翻译 | 分类／摘要 | 授权状态 | 当前判断 |
| --- | ---: | --- | --- | --- | --- | --- |
| Mozilla Bergamot `en→zh base-memory` | 36.75 MB 下载；49.91 MB 解压 | 5.31 MB WASM npm 包；在 Tauri WKWebView 中需要测量 | 专用任务；registry 有正式发布模型及 FLORES200+ 指标 | 不支持 | 代码仓库 MPL-2.0；权重再分发许可未从 registry 得到明确证明 | **最小翻译候选** |
| OPUS-MT / Marian `opus-mt-en-zh` | PyTorch 权重 312 MB；另有 578 MB Rust 权重 | Candle 可直接嵌入 Rust；Marian／CTranslate2 原生路线需要 C++ FFI 或 sidecar | 专用任务；模型卡有 en→zh 测试结果 | 不支持 | HF 模型卡写 Apache-2.0；OPUS-MT-train 对预训练模型写 CC-BY 4.0，需澄清 | 能跑，但被 Bergamot 的体积与产品集成路线压过 |
| Qwen2.5-0.5B-Instruct GGUF | Q2_K 415 MB；Q4_K_M 491 MB | `llama.cpp` 支持 Apple Silicon、Accelerate、Metal；需 C API/FFI 或非 HTTP sidecar | 可以生成，但本任务质量需要测量 | 可以尝试固定分类与摘要 | Apache-2.0 | **最小通用 LLM 评测候选** |
| Qwen3-0.6B GGUF | 官方当前 Q8_0 639 MB | 同上 | 官方明确提到 multilingual translation，但本任务质量需要测量 | 可以 | Apache-2.0 | 比 Qwen2.5 最小原型更重，暂不优先 |

表中体积只比较当前官方发布物。模型运行时、签名、通用 tokenizer、缓存和应用压缩会改变最终 `.app` 或下载体积，必须用真实构建产物测量。

## 方案一：Bergamot `en→zh base-memory`

### 已确认事实

Mozilla 当前的公开 [模型 registry](https://storage.googleapis.com/moz-fx-translations-data--303e-prod-translations-data/db/models.json) 为 `en-zh` 列出了 `base-memory` 发布模型：

- `releaseStatus` 为 `Release`；
- 参数量为 43,536,965；
- 权重采用 `intgemm`；
- registry 给出 FLORES200+ 的 chrF、chrF++、COMET22、spBLEU 等结果；
- 模型由权重、shortlist lexical table、源词表和目标词表四个文件组成。

本次直接读取 registry 指向的四个官方 GCS 文件并流式解压，得到：

| 文件 | 下载 bytes | 解压 bytes |
| --- | ---: | ---: |
| `model.enzh.intgemm.alphas.bin.gz` | 33,375,922 | 43,849,787 |
| `lex.50.50.enzh.s2t.bin.gz` | 2,536,039 | 4,485,184 |
| `srcvocab.enzh.spm.gz` | 407,784 | 806,952 |
| `trgvocab.enzh.spm.gz` | 425,748 | 772,004 |
| **合计** | **36,745,493** | **49,913,927** |

Mozilla 已归档的 [firefox-translations-models](https://github.com/mozilla/firefox-translations-models) 仓库说明模型针对 CPU 使用 `intgemm` 优化，并把当前来源指向 `mozilla/translations` 与上述 GCS registry。当前的 [mozilla/translations](https://github.com/mozilla/translations) 仓库说明这些模型用于 Firefox，并与 Bergamot 兼容。

[Bergamot translator](https://github.com/browsermt/bergamot-translator) 是基于 Marian 的跨平台 C++ 翻译 runtime，提供 native 和 WASM 构建。官方 [WASM 文档](https://github.com/browsermt/bergamot-translator/blob/main/wasm/README.md) 给出浏览器 JavaScript 接口；官方 npm 包 [`@browsermt/bergamot-translator`](https://www.npmjs.com/package/@browsermt/bergamot-translator) 可以用 `ArrayBuffer` 加载自定义模型文件。

### 本机真实 smoke test

在本机 Apple Silicon 上，使用官方 `@browsermt/bergamot-translator@0.4.9` WASM runtime 与当前 Mozilla `en-zh` Release 的四个文件，在隔离的临时目录中运行了 `node test-bergamot.mjs`。临时脚本和模型没有写入仓库。

| 英文输入摘要 | 实际输出 | 单次耗时 |
| --- | --- | ---: |
| `relentless interview...` | `一次无休止的访谈,以完善计划或设计。` | 240 ms（cold） |
| `hard bug...` | `当用户想要诊断硬错误或性能回归时,请使用。` | 19 ms |
| `spreadsheet...` | `在保存公式和格式的同时,创建、编辑和验证电子表格文件。` | 22 ms |
| `GitHub/pull request...` | `通过故意提交、推动分支并打开草稿取取请求,向 GitHub 发布本地更改。` | 28 ms |
| `dashboards...` | `构建由源支持的仪表板,用于监控性能并探索产品指标。` | 21 ms |

这组结果证明当前 WASM runtime、模型和模型配置可以在本机 Apple Silicon 的 Node 环境完成真实推理，warm 请求也很快。它同时暴露了不能忽略的质量问题：

- `pull request` 被错误翻译为“取取请求”；
- `source-backed` 被直译为不自然的“由源支持”；
- `relentless interview`、`committing intentionally` 等产品／工程语境仍需人工判断是否保留了原意；
- 输出使用半角标点，展示层是否需要标点规范化仍需决定。

这些耗时来自五条短文本的单次 smoke test，不是统计基准；没有记录峰值 RSS，也没有覆盖 Tauri WKWebView、Web Worker、应用签名或最低支持 macOS。它把“不知道能否推理”缩小为“已能在 Node 推理，但质量和产品 runtime 尚未验收”。

### 对 SkillYard 的适配判断

优先验证 WASM，不优先做 C++ FFI，原因是：

- WASM 不需要为 `aarch64-apple-darwin` 维护第二套 native ABI 和 Rust binding；
- 可以在 Web Worker 中隔离翻译计算，避免阻塞 UI；
- runtime 加模型约 55 MB，显著小于任何本次找到的通用 LLM；
- 翻译结果是派生展示数据，不需要进入 Lifecycle Core 的高风险文件操作能力。

但生产设计仍应由 Rust 控制可选模型的 manifest、下载、校验、版本、缓存和删除，前端不能获得通用文件系统能力。模型文件如何以窄接口安全地交给 Web Worker，需要通过 Tauri 原型确定；不能默认大体积 `ArrayBuffer` 经过 command 序列化没有成本。

### 必须测量

- Tauri 2 + macOS 14 WKWebView 是否能稳定初始化该 WASM；
- Web Worker、CSP、本地资源 URL 和应用签名后的加载行为；
- M1／8 GB 等最低支持设备的冷启动、首译时间、连续翻译吞吐和峰值 RSS；
- 一批真实 Skill `description` 中技术术语、命令、路径、品牌名和混合中英文的保真度；
- `zh` 模型是否稳定输出产品要求的**简体中文**，以及如何处理繁体或混合字形；
- 断网重启、模型损坏、下载中断、升级和删除后的行为；
- 模型权重的确切许可、NOTICE 和再分发条件。

### 能力边界

Bergamot 是翻译模型，不应让它“顺便”完成：

- 从描述推断产品分类；
- 重写或扩展作者没有表达的使用场景；
- 判断 Skill 是否安全、可信或适合安装；
- 推断 Source、发布者、安装命令或 Skill Identity。

这些边界与 Product Contract 的“模型不能替代确定性本地证据”一致。

## 方案二：OPUS-MT / Marian

官方 [Helsinki-NLP/opus-mt-en-zh 模型卡](https://huggingface.co/Helsinki-NLP/opus-mt-en-zh) 确认它支持 English→Chinese，并列出 Tatoeba 测试结果。官方 [文件树](https://huggingface.co/Helsinki-NLP/opus-mt-en-zh/tree/main) 中：

- `pytorch_model.bin` 是 312 MB；
- `rust_model.ot` 是 578 MB；
- 源、目标 SentencePiece 文件各约 0.8 MB；
- 仓库显示的 1.52 GB 包含 PyTorch、TensorFlow、Flax 和 Rust 等重复格式，不是单一 runtime 必须全部下载的大小。

它有两条可能的本地路线：

1. [Candle 的 Marian-MT example](https://github.com/huggingface/candle/tree/main/candle-examples/examples/marian-mt) 已展示 `en-zh` 推理，并且 Candle 官方说明可在 macOS 使用 Accelerate CPU backend。
2. [CTranslate2](https://opennmt.net/CTranslate2/guides/transformers.html) 支持转换 MarianMT；官方说明 macOS ARM64 wheel、Apple Accelerate 和 int8 量化。

但两条路线都不足以成为当前最小发布方案：

- Candle 的 en→zh example 使用模型仓库的非默认 PR revision，并从第三方仓库取得 tokenizer artifacts；
- example 虽暴露 `--quantized` 参数，但当前实现没有给出可核验的 en→zh 量化加载路径；
- CTranslate2 只有 C++／Python API，Tauri/Rust 需要 FFI 或 sidecar；
- CTranslate2 宣称 int8 最多可显著减小模型，但转换后的实际文件体积、质量和 Apple Silicon 性能均需测量，不能把理论比例当成本模型的实测结果；
- OPUS-MT 的官方来源存在授权表述冲突：HF 模型卡标记 Apache-2.0，而 [OPUS-MT-train](https://github.com/Helsinki-NLP/OPUS-MT-train) 说明预训练模型按 CC-BY 4.0 分发。再分发前必须澄清准确来源和适用许可。

OPUS-MT 仍可作为 Bergamot 的质量对照组，但不应成为默认实施路线。

## 方案三：Qwen 小型通用 LLM

### Qwen2.5-0.5B-Instruct

官方 [Qwen2.5-0.5B-Instruct-GGUF 模型卡](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF) 说明：

- 0.49B 参数，其中 0.36B 为非 embedding 参数；
- 支持包括中文、英文在内的 29 种以上语言；
- 强化了 instruction following、长文本生成和 JSON 等结构化输出；
- 使用 Apache-2.0。

官方 [GGUF 文件树](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/tree/main) 给出的主要量化体积是：

| 量化 | 官方文件体积 | 判断 |
| --- | ---: | --- |
| `Q2_K` | 415 MB | 最小官方文件；质量风险需要测量，不推荐直接发布 |
| `Q4_0` | 429 MB | 比 Q4_K_M 小，但没有本任务证据 |
| `Q4_K_M` | 491 MB | 官方 quickstart 选择；最小合理评测起点 |
| `Q5_K_M` | 522 MB | 更大，除非评测证明有必要 |
| `Q8_0` | 676 MB | 对“最小本地模型”没有优势 |
| `F16` | 1.27 GB | 不适合该体积目标 |

Qwen2.5-0.5B 可以尝试用固定 schema 返回：

- 原文对应的简体中文翻译；
- taxonomy 允许范围内的一个或多个分类；
- 不增加事实的短摘要；
- 证据不足时的 `unclassified`。

但“能生成 JSON”不等于“分类正确”，“支持中文”也不等于“英文到简体中文翻译足够可靠”。在真实 Skill 语料通过盲评前，491 MB 只能称为原型候选。

### Qwen3-0.6B

官方 [Qwen3-0.6B 模型卡](https://huggingface.co/Qwen/Qwen3-0.6B) 明确提到 100 种以上语言以及 multilingual instruction following 和 translation，许可证是 Apache-2.0。当前官方 [Qwen3-0.6B-GGUF 文件树](https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/tree/main) 提供的 `Q8_0` 文件是 639 MB。

它对“支持翻译”的官方表述比 Qwen2.5-0.5B 更直接，但：

- 官方卡没有给本次短技术描述 en→zh 的具体质量结果；
- 当前官方 GGUF 比 Qwen2.5 的 Q4_K_M 更大；
- 自行生成 Q4 量化后的准确体积和质量都需要测量。

因此它适合作为质量对照，不是最小候选。

### Apple Silicon 与 Tauri 路径

[llama.cpp](https://github.com/ggml-org/llama.cpp) 使用 MIT License，官方支持 ARM NEON、Accelerate 和 Metal；[macOS 构建文档](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md) 说明 Metal 默认启用。它可以通过 grammar 约束 JSON。

对 SkillYard 有三种集成形态：

1. `llama.cpp` C API + Rust FFI：不启动 localhost，但需要维护 native build、ABI、签名和崩溃边界。
2. 随 `.app` 打包一个 arm64 sidecar，以 stdin/stdout 通信：Tauri 官方 [sidecar 文档](https://v2.tauri.app/develop/sidecar/) 支持按 target triple 打包二进制；仍需新增进程生命周期、权限、签名和故障恢复设计。
3. `llama-server` localhost API：实现最容易，但与当前 Product Contract 的“生产不启动 localhost Server”冲突，**排除**。

哪一种都比 Bergamot WASM 多出明显的 runtime 与发布复杂度。`llama.cpp` binary 的最终体积、M1／8 GB 的 RSS、首 token 时间和每条 Skill 总延迟均需用真实 release build 测量。

## 推荐的最小产品设计

### 只解决自动翻译

采用“原文为事实、译文为可丢弃派生缓存”的设计：

1. UI 永远保留作者原始 `description`，翻译失败时直接回退原文。
2. 用户显式启用本地翻译后才下载／加载模型；下载前展示准确体积、来源、许可和删除入口。
3. 缓存键至少包含原文内容摘要、模型 id、模型版本和目标语言；模型升级不覆盖旧事实。
4. 翻译不阻塞扫描、安装、更新或打开 Skill 列表。
5. 不把译文写回 `SKILL.md`，不改变 Source 内容，也不把译文用于 Skill Identity 或安全判断。
6. 首选 Bergamot WASM；native C++ 只在 WKWebView spike 失败后再评估。

### 同时解决分类和摘要

先明确 Issue #43 是否真的需要“模型生成摘要”：

- `description` 本身已经是作者提供的 1–1024 字符概要；很多情况下只需翻译和更好的 UI 呈现，不需要模型重写。
- 分类应先固定 taxonomy、允许多标签、`unclassified` 和人工纠正语义，再评测模型；不能让模型自由发明分类。

只有当固定评测证明 NMT + 确定性分类不足时，再用 Qwen2.5-0.5B-Instruct `Q4_K_M` 做原型。不要为了复用一个模型，让所有用户承担约 491 MB 权重、通用推理 runtime 和更高内存成本。

## 发布前验收

### 共同样本集

从真实可发现或已安装的 Skill 中冻结一份评测集，至少覆盖：

- 普通英文描述；
- 已经是中文和中英混合的描述；
- CLI command、API、文件路径、model name、缩写和品牌名；
- Markdown、引号、反引号、变量名和多行文本；
- 否定句、条件句、多个使用场景以及信息不足的描述。

样本必须保留人工参考翻译、固定分类和审阅记录。模型版本、runtime 版本、prompt、量化和设备都要进入评测结果，避免“换了模型但沿用旧结论”。

### 翻译硬门槛

- 不崩溃、不修改原文、不阻塞主流程；
- 不删除或新增关键能力、限制条件和否定含义；
- command、路径、API 和专有名词不被错误翻译；
- 输出满足简体中文要求，或明确回退原文；
- 在最低支持 Apple Silicon 设备上记录冷启动、首译、批量吞吐、峰值 RSS 和磁盘占用；
- 断网重启后不访问模型网络来源；
- 权重来源、校验摘要、许可、NOTICE 和删除行为均可核验。

具体可接受的质量百分比和性能预算应由产品验收样本确定，本调研不伪造一个没有实测依据的阈值。

### 通用 LLM 追加门槛

- 输出 100% 通过 schema 校验，不在 taxonomy 外发明标签；
- 证据不足时能够稳定返回 `unclassified`，而不是猜测；
- 翻译、分类和摘要分别评测，不能用“看起来整体不错”代替分项结果；
- 与“Bergamot 翻译 + 无模型分类／确定性分类”的基线比较实际收益；
- prompt injection 式 `description` 不能越过固定输出约束，也不能触发文件、网络或 shell 能力；
- 记录相同设备上的权重加载时间、峰值 RSS、总延迟和电量影响。

## 不推荐

- **Qwen2.5 `Q2_K` 直接发布**：415 MB 只是最小文件，不是已证明的可用质量下限。
- **为纯翻译引入 Qwen**：权重至少约 415–491 MB，任务却可以由约 55 MB 的专用 NMT 候选完成。
- **Qwen3-0.6B Q8 作为最小方案**：639 MB，当前没有证明它在本任务上值得增加的体积。
- **Transformers + PyTorch 打包进 Tauri**：模型权重已经较大，还会引入与 1.0 边界不相称的 runtime。
- **Python sidecar 或 localhost inference server**：与当前 Product Contract 直接冲突。
- **直接使用 OPUS-MT 的 312 MB F32 权重**：体积和集成成本都弱于 Bergamot，且授权表述需要澄清。
- **把 Bergamot 代码仓库的 MPL-2.0 当作模型权重许可**：registry 未给出足够证据，当前不能这样下结论。
- **先做 native C++ FFI**：Bergamot 和 CTranslate2 都没有本调研可确认的官方 Rust API；先验证 WASM 能否满足产品更简单。

## 最终决策表

| 问题 | 答案 |
| --- | --- |
| 本地自动翻译最小能跑的模型是什么？ | Mozilla Bergamot `en→zh base-memory`：36.75 MB 下载，模型 + WASM runtime 解压约 55.23 MB。 |
| 本地自动翻译最小值得发布的模型是什么？ | 同一个 Bergamot 模型是最小发布候选；但在权重再分发许可和 Tauri/macOS 14 实测完成前，当前没有已证明可直接发布的模型。 |
| 如果还要同一模型负责分类和摘要呢？ | 从 Qwen2.5-0.5B-Instruct `Q4_K_M`（491 MB）开始评测；它是最小合理原型候选，不是已证明的发布下限。 |
| 自动翻译能独立解决 Issue #43 吗？ | 不能。它只解决理解英文 `description`；分类与使用时机仍需独立产品定义和验证。 |
| 现在应该实现吗？ | 不应直接进入 SkillYard 1.0；先确认后续版本边界，再做 Bergamot Tauri spike、真实语料评测和模型权重许可核验。 |
