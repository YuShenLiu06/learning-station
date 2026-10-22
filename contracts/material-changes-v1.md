# 固定版本 Changes

本层基于 Focus 9440b51 的课程路由接入 generation：`for_focus` 只消费 `focus::load` 的同一 Frozen，经 `focus::open_materials(core, focus)` 读取固定 OID，不再次 capture 或刷新目录。完整产品验收仍以组合测试和对应 SHA 的 CI 为准。

底层职责已并入 versioning 接口（[接口契约](../docs/file-metadata-and-versioning.md)）：单文件与课程级统计、diff 与固定版本读分别由 `file_stat`/`all_stat`/`file_diff`/`all_diff` 提供，本层不再另开 Git 或哈希路径。独立 Changes 桥部件将并入 Focus 部件（Layer 2 待实现）：课程级变化作为 Focus 部件的课程变化块消费 `all_stat`，私有管道与接收语义随 Layer 2 重组；Layer 1 现状：material_changes 消费链与私有管道保持不变。

## 固定比较与正文

`material_changes::prepare` 只复用 MaterialGit.checkpoint/files/read/compare，不 capture、commit、读取最新工作区或记录模型接受。调用者冻结课程、NEW OID、目标来源身份与安全相对路径。

`baseCommitOid` 是本 session 实际请求核对后的课程变化通知 OLD，用于其他文件的 name-status/numstat。`focusBaseCommitOid` 是当前有效完整正文投影的 OLD；聚焦 patch 单独比较这个 OLD 和冻结 NEW，条目的 patchBaseCommitOid 及 focused.gitChange.baseCommitOid 明示其基准。A→B→A 不把 B 时收到的课程通知当成 A 正文。基准由 Pi 现有材料条目提供，Rust 不拼历史或另建缓存。

返回 checkpoint 保留 courseId/checkpointCommitOid/materialTreeOid/excludedFiles，完整 focused 保留真实来源 id/path、内容 SHA version、独立 gitCommitOid/gitBlobOid、text/textHash/range。全文是固定 Git 文件的原始 UTF-8 文本；range 为此文本的字符坐标。Focus 可见段落和 Citation 的正文坐标不能仅凭同 sourceId/version 就假定与含前置信息的 Git 文件坐标相同；`focused.bodyStartChar` 从顶部元数据解析后的真实正文边界计算并传到 Pi 材料投影；文本附件为0。Citation补充切片须在其已提交head上按真实范围组合，去重保留双来源。

非聚焦只给文件名/name-status/numstat。聚焦完整原生 Git patch 只由 focused.gitChange 交给 Pi 材料帧；changes 通知保留统计及 patchBaseCommitOid，不复制 patch。容量检查也不计算重复副本，不截断完整 patch；非 UTF-8 patchBytes 保留在材料服务，发送层明确 patchUnavailable/patchByteLength，不乱码替换或丢文件。二进制只文件状态，通用计算从固定NEW checkpoint原生Git分类取得，不用NUL猜；最终wrapper仅对markdown/text且!Focus.binary提供正文，沿封存类型边界。主动受支持read仍走正常权限和工具结果，不用diff缓存替身。

fixedVersion 只含课程、固定 commit OID 和材料相对路径；删除指向删除前版本。附件路径取固定 files/read 的真实安全投影，不拼主机路径。排除清单保留真实文件名/原因；未注册文件可没有 assetId，清单仍逐项保留独立身份，不捏造可读 sourceId。

全包512KiB、来源清单256项超过预算显式 LIMIT_EXCEEDED，不宣称不完整材料已完整接收。

## 私有管道与接收

Rust 只有注册 material_changes handler 才发 materialChanges=true；Node 才启用私有 material_changes {id,baseCommitOid,focusBaseCommitOid}，每轮最多8次，含压缩恢复。能力位不注册模型工具或扩大 read 权限，前端/模型不能指定课程、NEW OID、目录或任意路径。无 provider 的旧 Focus 可继续已有 nativeMaterials 投影，便于正常依赖组合。

材料通知不等于正文。Pi 沿当前 session 的实际 full→diff 链验证完整范围、身份、hash 和 Git blob/commit。首次、新 session、压缩或正文基准失效恢复真正全文，不能裸 diff；无变化不追加正文/通知。回合 Focus 元数据只保留定位/范围，不重复已交给材料投影的 text，原冻结对象不变。

Changes 消息与原 request/run/user/native entry 绑定。实际准备请求含该条目后追加接收记录并 session_sync；这证明本地 Pi 请求投影，不宣称服务端已读。失败、取消或未知沿原身份核对；未启动重试不能推进，取消也不一律等于未收到。本地 Git commit、courseReadManifest 授权元数据、非聚焦 summary 都不能推进正文基准。

full commit、材料 tree、课程仓归属与各 session 接收身份分开。本层不提交聊天或做联合回滚。

验证只使用合成课程、真实本地 Git、锁版 Pi 和 localhost。固定计算、Node资源/正文投影、实际私有管道分别报告；同一 Frozen/generation 接缝与前端/build分别验证；已正常组合Citation be33fd5，七轮实际请求验证其materialRange和双来源、Focus元数据去正文及压缩恢复；最终SHA CI与本人GUI验收仍须独立完成。
