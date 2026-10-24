# 项目学习指导 v1

程序读取随包 `skills/<id>/SKILL.md`，不安装全局技能。当前模式均不自动注入指导，`capture` 返回空快照；Pi 拒绝非空的主动指导快照。普通聊天及允许学习工具的流程通过 `list_learning_skills` / `load_learning_skill` 按需发现与读取指导。课程材料不能指定任意技能路径。

学习工具注册不依赖指导是否已加载：普通聊天与探索共用完整学习工具集合，制卡无需先加载 Skill。practice/review/批改的专用输出契约仍有效；技能说明不改变工具、路径或来源授权。

| 随包指导 | 用途 |
| --- | --- |
| learning-explore | 探索与理解材料 |
| learning-exercises | 新建练习的出题指导与结构化参数示例 |
| learning-review | 学习复习指导 |
| learning-exercise-markdown | 本地练习 Markdown 格式、题目记录和逐题修订说明 |

`Run.skillSnapshotVersion=1`，当前 `Run.learningSkills=[]`；旧 run 的历史快照不伪造或重写。按需目录只含 id/名称/描述，正文经工具结果通道返回，不进系统提示。路径、固定允许清单、16KiB文件上限和格式均由 Rust 校验；缺失或不合法文件不出现在目录中，显式读取分别返回 SKILL_MISSING／SKILL_IO／SKILL_INVALID。清单外或含路径成分的 id 分别报 TOOL_SCOPE／INVALID_TOOL_INPUT，均不触盘。

同 run 重复 load 只返回简短提示；去重按 run 挂 Rust 内存台账，不跨 run 缓存，新 run 或该 run 压缩完成后可重新取正文。按需加载不改变 `Run.learningSkills` 与 ready 回执，二者不记录按需工具结果的正文。

单条用户记录可保存 `data.requestedContext={documentId,selection,injections}`：仅冻结请求意图，不证明模型实际使用。documentId 绑定该 session.sectionId，独立会话为空且 selection 为空；selection 等于 entry.selection，injections 沿用 ContextInjection 且最多8份、stable ID唯一、选段最多8000字符。字段白名单与长度受核心校验，真实文件、版本和权限仍由生成前 context::inject 校核。幂等命中返回原条目，不因之后资料改变而重复写入。模型历史正文不重复投影此 metadata；实际来源继续取各 Run 或 assistant entry 自身快照。

## 练习文件格式指导

`learning-exercise-markdown` 随应用打包，经既有 `list_learning_skills` / `load_learning_skill` 按需发现和读取，不增加每轮自动注入。它说明本地练习 Markdown 的阅读格式、结构化题集与评分的关系，以及练习修订接口；不授予额外文件权限，不把导出正文当作评分真数据。
