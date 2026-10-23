# 做题页分区引用修复契约

范围仅为做题页渲染选段、快捷生成引用生命周期及对应导航/停止UI bugfix，不含自动focus、制卡白名单、文件/联网/命令工具。本轮内部验证，不单独交付应用。

只读`get_exercise_reference({sessionId,documentId,questionId,part,entryId?})`返回`{sessionId,documentId,exerciseEntryId,questionId,part,entryId,title,documentVersion,text,textHash,sourceId}`。

- part为question/attempt/feedback/reference。question/reference的entryId是原exercise entry；attempt/feedback必须是真实持久记录且属于该题，反馈还核对应attempt。
- 题干取原问题Markdown，作答取本人已存原答案，反馈仅原持久内容及AI公开评语/批注/遗漏/重点字段；reference取`answers.join(' / ')+'\n\n'+explanation`，仅真实提交该题后有资格。UI仅在实际展开的参考区域采集；未提交草稿不能伪作持久记录。
- 渲染选段须来自同一题、同一分区且为规范原文连续串。先复用公共渲染到原文定位；无法精确匹配时在做题页展示规范原文重选，不能切编辑页、拼跨分区或信DOM文字作为原文。
- 发送`exerciseReferences?:[{sessionId,documentId,exerciseEntryId,questionId,part,entryId,textHash,selection,locator?}]`，textHash为该规范全文的UTF8 SHA256小写64位。与材料及AI引用合计最多8份，每段8000码点/30000字节，文本总预算60000码点；同分区不重复。
- load_session_context/start_generation/用户requestedContext一致；Core创建practice user也冻结此数组。重试原user不能换引用，append原编号已存回执优先，未知不盲重发。
- 来源单列Context.exerciseReferenceManifest、Run.exerciseReferenceManifest、Session.contextExerciseReferenceManifest及assistant自身data.exerciseReferenceManifest。格式ManifestItem，id为sourceId，path为`exercise-part/<documentId>/<questionId>/<part>/<entryId>`、version为textHash。不是教材，不进入工具allowedIds。
- Core只读当前session所属exercise子文档和精确分区，核元数据、题/记录绑定、公开资格、全文hash及选段；绝不读整份渲染Markdown进入请求。原文变化CONTEXT_CONFLICT、外会话/未公开OUT_OF_SCOPE、缺失NOT_FOUND、数量/预算LIMIT_EXCEEDED，结构非法INVALID_CONTEXT_REQUEST。

来源快照属于本次请求，历史不能用Session最新字段回补。成功落盘后按现有revision票据单次消费，失败重试仍原快照，新快捷生成只用当前未消费队列。

公共位置和辅助上下文决策见 [Citation 切片契约](citation-v1.md)；quotedText 只含原文选区，不默认追加参考答案或整题。
