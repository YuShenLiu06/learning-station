# 项目学习指导 v1

程序读取项目 `skills/<id>/SKILL.md`，不安装全局技能。每次新生成在取凭据前固定完整文本与 SHA-256，私有管道传给 Pi；Pi 校验模式、范围和哈希，主动展开文本并返回相同版本回执。模型不能从资料指定技能或路径。

| 模式 | 指导与注入范围 |
| --- | --- |
| question（探索） | learning-explore，全量进入该 run 的固定系统提示 |
| practice（原练习按钮） | learning-exercises，全量进入系统提示，保留现有结构化输出 |
| review（明确的 AI 复习请求） | learning-review，全量进入系统提示；仅关联知识查询工具 |
| question／explain 中注册 generate_exercises | 同一 learning-exercises 全量进入该工具说明，只指导出题，不覆盖普通回答 |
| 连接测试、纯 Anki 翻卡/四档评分、现有批改 | 不额外启动 AI，不加载这三份指导；现有批改仍单次零工具固定 JSON |

`Run.skillSnapshotVersion=1`，`Run.learningSkills=[{id,sha256,scope}]`；scope 为 `mode` 或 `tool:generate_exercises`。同一 run 不再读取当前文件；旧 run 缺字段按未记录处理，恢复不伪造版本。必需文件缺失、不可读、格式或哈希不符分别报 SKILL_MISSING／SKILL_IO／SKILL_INVALID，不能静默继续。出题指导按 ID 去重，不把三份指导注入每次请求。

单条用户记录可保存 `data.requestedContext={documentId,selection,injections}`：仅冻结请求意图，不证明模型实际使用。documentId 绑定该 session.sectionId，独立会话为空且 selection 为空；selection 等于 entry.selection，injections 沿用 ContextInjection 且最多8份、stable ID唯一、选段最多8000字符。字段白名单与长度受核心校验，真实文件、版本和权限仍由生成前 context::inject 校核。幂等命中返回原条目，不因之后资料改变而重复写入。模型历史正文不重复投影此 metadata；实际来源继续取各 Run 或 assistant entry 自身快照。
