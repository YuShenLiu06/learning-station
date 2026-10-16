# 核心边界

| 部分 | 权威数据与职责 |
| --- | --- |
| content/context | 课程与知识点 Markdown；稳定 ID 关联、受限上下文和来源版本清单 |
| sessions | 每会话 Markdown 及每条学习记录；作答反馈、可修正错因与复习导出 |
| model/generation/learning_tools | Pi sidecar与按模式工具白名单；Rust复用学习记录/复习服务；用途门槛、流、取消和生成状态 |
| reminders/feishu | 独立运行目录的 SQLite 通知 ledger；去重与固定本人通道适配 |
| drafts | 仅允许正文／问题／导入内容的本机草稿，不接收模型密钥 |
| desktop | Tauri IPC 和窗口事件，异步执行磁盘工作 |

课程 `library/courses/*.md` 顶部是 JSON 围栏，正文可直接编辑；个人记录在 `library/sessions/`。索引可从 Markdown 重建。运行状态、保存历史、草稿和通知 ledger 存应用数据目录，排除 Git。保存按当前 SHA 校验，先保存原文快照，再原子替换。

上下文只沿稳定 ID 链接加载，拒绝路径穿越与符号链接越界，最多32份、4层关联、60000字符，并给出遗漏/截断提示。资料和学习历史以引用资料进入 user 消息，不升格为 system；历史输入另有20000字符上限。

提示片段仅用固定版本标记追溯，参考[PromptStore](https://github.com/YuShenLiu06/PromptStore)的整理思路，未引入其框架或复制其实现。没有接入 DSH 框架，也不声称已经符合待确认论文的完整架构。
