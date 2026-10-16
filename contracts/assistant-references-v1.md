# 本次引用与 AI 回答引用

状态：下一版引用增强契约；真实在线服务与原生界面未测。

- `load_session_context`、`start_generation` 追加可选 `assistantReferences`，省略等同空数组。
- 每项 `{sessionId,entryId,runId?,textHash,selection}`。只当前会话真实 `assistant`；`textHash` 为整条原始 Markdown 的 UTF-8 SHA-256 小写64位。原记录有 `data.runId` 时必须一致，旧无 runId 只能省略。selection 是非空连续原文。
- AI 回答最多4条；与 `injections` 合计最多8份。同回答不重复；每选段最多8000码点/30000字节；引用文本与材料受60000码点/32来源总预算约束。预算不足整段拒绝，不悄悄截短。
- AI 文本以不可信历史引用发送，不能成为指令、教材或工具权限。材料 `manifest` 保持原义，AI 引用不扩大工具可访问资料 ID。
- `Context.assistantReferenceManifest`、`Run.assistantReferenceManifest`、`Session.contextAssistantReferenceManifest` 单列真实本次来源；`ManifestItem` path 为 `ai-answer/<sessionId>/<entryId>`，version 为全文哈希、includedChars 为所引选段码点数。
- 每条 assistant 自身 `data.assistantReferenceManifest` 固定当次快照，历史不得借最新 Session 补齐；旧字段缺失显示未记录。
- user `data.requestedContext` 在原字段后加可选 assistantReferences。Core 创建 practice user 也保存同一冻结清单。意图不等于模型实际使用，实际来源看对应 Run / assistant 快照。
- 原 userEntryId 重试必须保持冻结引用一致；未知结果先核原 operation。`append_entry` 原编号幂等回执优先，已成功落盘的旧问题不会因后续源变化而重复保存。

错误：外会话/非assistant `OUT_OF_SCOPE`；自己的来源消失 `NOT_FOUND`；全文/运行编号/选段变化或重试清单不符 `CONTEXT_CONFLICT`；数量/预算 `LIMIT_EXCEEDED`；结构非法 `INVALID_CONTEXT_REQUEST`。回收来源不能经历史AI引用绕过。

发送体验：仅真实 user 落盘确认后消费本次手动引用；失败未确认保留；模型失败后消息仍带原冻结引用可重试。迟到回执不清后来新增的引用/问题。practice 要从真实 get_session 核对 user 的 ID、kind、内容、mode、run 归属后才消费，Run 预分配 ID 不算保存成功。必要模式材料不消费。
