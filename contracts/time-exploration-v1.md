# 探索场景与请求时间 v1

从固定探索目录或旧探索进入的 question 请求，由核心确定场景，附原始问题、本轮目标、存储笔记 ID 和真实关联材料 ID。Pi 的可信系统提示直接展开“问题发起的探索”说明；空问句笔记是存储容器，不能被当作缺失教材。材料仍按原范围引用，问题和回答已由应用保存，不增加逐条存储确认。资料内容与用户目标仍是引用，不成为指令；删除、覆盖和外发权限不变。

每个实际 AI 请求均由程序注入 referenceTime（UTC ISO）、IANA timeZone、当地 utcOffset 和 localDateTime，适用于探索、讲解、练习、复习、反馈、批改和连接测试。采用本机实际时区（有效 TZ 或系统 IANA），不硬编码 UTC+8；未知时区明确 unknown，不能冒称精确当地时间。Node 依据该 IANA 的时区规则计算夏令时。

Run/GradeRun 持久 referenceTime 与 timeZone，Run 另存 explorationContext；旧记录不伪造。state/time-anchors 按真实 userEntryId（无 entry 时按请求 ID）固定解析锚点，跨 run 的同问题重试继续使用；新的用户问题采用新时钟。损坏/不可读不能当缺失重建。Pi ready 回执须确认同一时间锚点。

create_todo 增可选 relativeMinutes（1–525600 整数），优先用于“10 分钟后”等经过时长。Pi 按固定锚点和 IANA 生成带偏移的 dueAt，Rust 独立重算绝对时刻并核验两者相等，不一致不写入；回执含 dueAt/referenceTime/timeZone，原“打开待办”入口可核对真实截止时间。绝对 dueAt 仍须合法 RFC3339 时区/偏移。提醒默认关闭；仅当本轮用户明确要求提醒时模型可传 reminderEnabled=true，启用必须带提醒时间且晚于本轮 referenceTime，缺失/非法/过去时间拒绝，回执的 todo.reminderEnabled 反映实际调度状态。

飞书到期消息仅在原调度到点后，用纯代码组成“小主人，你的任务「标题」到时间啦，记得处理哦。\n到期时间：当地日期时间及偏移”。不新增提前提醒，不称“即将到期”。测试消息保持独立测试文案；标题、通知编号和 unknown 去重不变。用户已确认旧包固定测试消息与到期待办链路成功，不能替代本版模板和 WS 入站的真实验证。

本次不接入一次性引用消费或 AI 回答引用；下一版独立协调，不覆盖当前运行包。
