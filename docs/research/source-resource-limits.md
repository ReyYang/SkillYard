# Source 资源限制依据

> 研究快照：2026-07-21。本文记录 SkillYard 1.0 固定资源上限的实测依据，不负责修改产品契约。

## 1.0 固定上限

| 计数项 | 上限 |
| --- | ---: |
| 累计接收字节 | 100 MiB（104,857,600 bytes） |
| Archive 条目数 | 20,000 |
| 展开后的普通文件总字节数 | 512 MiB（536,870,912 bytes） |
| 单个普通文件 | 100 MiB（104,857,600 bytes） |

达到上限本身是合法状态；下一次读取、创建 Archive 条目或写入文件会超过上限时，Adapter 必须立即停止并拒绝候选内容。所有网络和 Archive Adapter 使用同一组常量，用户不能按 Source 放宽限制。

## 实测基线

对 1.0 四个推荐 GitHub Source 的分支 Archive 与递归 Tree metadata 进行测量后，观察到的最大值为：

| 指标 | 最大实测值 |
| --- | ---: |
| 分支 Archive | 21,250,461 bytes |
| 递归 Tree 文件内容总量 | 32,685,489 bytes |
| Tree 条目数 | 2,048 |
| 单个文件 | 3,335,717 bytes |

测量对象：

- [anthropics/skills](https://api.github.com/repos/anthropics/skills/git/trees/main?recursive=1)
- [ComposioHQ/awesome-claude-skills](https://api.github.com/repos/ComposioHQ/awesome-claude-skills/git/trees/master?recursive=1)
- [cexll/myclaude](https://api.github.com/repos/cexll/myclaude/git/trees/master?recursive=1)
- [JimLiu/baoyu-skills](https://api.github.com/repos/JimLiu/baoyu-skills/git/trees/main?recursive=1)

这些数值不是生态规模上限，也不能证明任意未来 Source 都适配；它们只说明当前固定值对推荐来源留有明显增长空间，同时能为异常下载和 Archive 展开建立确定边界。
