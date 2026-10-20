# 显式 Citation 补充上下文 v1（#13 补齐中）

本契约覆盖原 #13 已要求、PR46 精确选区底座尚未交付的扩段/练习状态/Focus去重。旧e0bb544与运行中验收app保留对照；本轮完成并组合验证后正常追加更新同PR46，不另交半份验收。实现与组合仍在进行，不能称本人通过。

## 冻结数据与生命周期

现有 `injections/assistantReferences/exerciseReferences` 仍是用户引用意图。Rust新增 `Context.citationContext` 及已保存user的服务端 `data.citationContext`：

```ts
type CitationContext = { version: 1; references: Array<{
  source: { id: string; path: string; version: string; includedChars: number };
  quotedText: string; locator: { startChar: number; endChar: number } | null;
  supplementalContext: Array<{
    source: { id: string; path: string; version: string; includedChars: number };
    range: { startChar: number; endChar: number };
    requestedRange: { startChar: number; endChar: number };
    totalChars: number; totalBytes: number; text: string; truncated: boolean;
  }>;
  exerciseState?: { questionId: string; status: 'unanswered'|'submitted'|'graded';
    attemptId: string|null; feedbackEntryId: string|null };
}> };
```

位置均为原始来源的Unicode码点半开范围；path是来源说明，不是任意主机路径授权。材料version沿冻结Document.version，AI沿稳定session/entry/run与消息全文hash，练习沿规范part全文hash及持久entry ID；Git检查点仍由Focus/Git契约单独记录，不把文件hash称Git OID。

`quotedText`永远是精确原文。补充单列、不扩写该字段。新user拒绝客户端提供服务端字段；`sessions::append`先核原requestDigest落盘回执，再冻结/保存补充。`append_citation`内部入口接受同次已验证Context快照，供快捷练习/Focus复用，不能重新采样状态。原user恢复沿原快照；旧user缺此字段不反向补新答案。

## 来源适配与预算

- 普通笔记/AI消息：按真实源文本空行段落和代码围栏边界补全所选块，最多前后各一个相邻普通段落，标题、列表/表格/HTML/围栏边界阻断相邻扩展；不把DOM显示文本当版本或追加整篇。重复文字必须精确定位；旧无locator且多次匹配时不猜扩段位置。
- 练习：复用 `exercise_references::project/context_parts`。同题题干、真实已保存attempt和作答状态单列；选旧attempt/feedback绑定那次attempt，其余绑定冻结时最新保存attempt。参考/解析只在同题实际提交资格满足时读取；反馈只使用既有public_feedback白名单。私有草稿、原始评分prompt/rubric/diagnostics不进入补充。
- 沿每引用8000码点/30000字节、整轮60000码点/32实际来源预算。精确选区优先；补充不足时按真实范围裁剪并标truncated，不能静默截断quote。模型实际裁剪与持久用户原快照分开，实际来源清单记录实际范围与截断。

决策样例：五个空行分隔的普通段落为“远前 / 近前 / prefix chosen suffix / 近后 / 远后”时，`quotedText`只保留`chosen`；补充只包含近前、所选段落其余部分与近后，远前/远后不进入。遇标题、列表、表格或代码围栏不向该块之外扩相邻段落。预算不足优先保留靠近选区的真实范围，显式标截断，不另设token魔数或新增设置UI。引用同题旧作答则补充该次真实attempt/status，不能把之后提交的新答案混入历史。参考 [Continue选区与文件上下文](https://docs.continue.dev/ide-extensions/edit/how-it-works) 和 [Cline显式文件引用](https://docs.cline.bot/core-workflows/working-with-files) 的选区/来源区分；本项目使用上述保守段落边界，不照搬整篇注入。

## Focus / Changes / Pi 接缝

- 授权元数据 `courseReadManifest`、非聚焦summary、Git文件存在均不是正文覆盖。Citation只消费同次 `Context.automatic_materials` 的**真实正文**（来源ID、Document.version及精确字符范围/原文一致）或同轮更早Citation已投影范围。
- 坐标必须区分：`MaterialSnapshot.body_start_char` 在真实Document采集时按`content`与`body`的已解析边界冻结；正文-only/文本附件为0，不能从DOM或找第一次相同字符串猜偏移。nativeMaterials输出`bodyStartChar`；Citation自身range仍是正文坐标，`textFrom.materialRange`指向真实材料投影坐标（可含顶部元数据）。旧私有快照缺字段默认0且0省略序列化，保留原指纹兼容；没有可靠正文覆盖时发原文而非裸指针。
- Focus首次无有效正文基准时必须full；后续同session只有Pi原生projection确认full→固定OLD/NEW Git OID链仍有效，才用完整聚焦diff/无变化不重复。新session、压缩/基准失效重载full。Citation不创建接收缓存、不推进receipt、不改Pi加载/压缩策略。
- 模型侧去重只改变投影：原快照保留quotedText/text，模型已覆盖部分以 `quotedTextProjection` 或补充项 `projection` 的 `{range,textFrom:{provider,sourceId,citationId,version}}` 指向真实材料/更早引用；未覆盖部分仍发text。保留Focus/材料清单与Citation清单两种来源标记，不能仅留下引用指针却丢失正文。
- `citation_context::quote`按同次真实范围去重并裁补充；桥应将补充source/range/truncation列入实际清单，实际Pi manifest回执继续落到Run/该assistant自身，不从最新Session回填旧消息。
- **Focus集成必须复用同一冻结值**：`freeze_focus`的Core锁内先校验/生成CitationContext并纳入私有`Frozen.context`指纹和固定检查点。保存user走服务端已验证的 `focus::load(...).context.citation_context` → `sessions::append_citation`，不信客户端快照，也不再次从工作区采样。generation加载同一Frozen并核user快照一致；固定OID内容来源验证交既有Focus/Git适配器，不能用之后工作区正文替代。
- live无Focus兼容路径使用现有store身份/版本校验；组合后应注入已验证固定来源视图，禁止为了通过校验重绑最新版。课程root/multi-session只沿 #54 真实session/source解析，不复制其未提交接口。

## 必须验证

精确quote不扩写；段落/围栏/CRLF/emoji/重复句；补充预算与真实范围；无提交不带参考/草稿，已提交同题真实答案/状态，旧attempt不混新答案；伪造/跨题/跨会话/回收/过期拒绝；原保存回执/未知/取消/快捷生成与旧历史不重采样；同源全文/部分范围/异版本/异ID去重；metadata-only/summary不得作为正文；新session/压缩失基准恢复full；固定Focus/Git/课程接口组合后再交付。
