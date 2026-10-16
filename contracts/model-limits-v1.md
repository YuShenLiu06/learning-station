# 模型输出预算（内部实现）

`set_model_limits({maxOutputTokens: integer, contextWindow: integer | null}) -> ModelSettings`

- 必须先保存连接；只改当前保存绑定的非秘密预算，零网络、零密钥访问，保留 provider/protocol/baseUrl/model/verified。
- 输出为 1–131072；上下文未知留空/null，填写为 1024–2000000，且 `maxOutputTokens <= contextWindow - 1024`。不合格返回 `INVALID_MODEL_LIMITS`，原配置不变。
- 生成或批改 queued/running 返回 `BUSY`；未配置返回 `NOT_CONFIGURED`。
- `ModelSettings.maxOutputTokens` 返回实际预算；旧配置 custom=4096，GLM=32768。`contextWindow=null` 表示未知，不将旧硬编码值当模型规格。
- 相同 endpoint/protocol/provider 的模型名切换保留预算；改变绑定不自动迁移。不同模型的实际支持值仍需用户核对。
- 生成、练习与批改使用开始时捕获的预算；连接测试至多 custom=256/GLM=2048，用户预算更小时按更小值。Pi 真实协议 payload 的 `max_tokens` 与该预算一致。
- 预算可能包含思考 token。1024 是配置预留校验，不代表已精准计算材料 token；不自动探测能力、不自动拉满或重试，服务仍可拒绝超出其真实支持范围的请求。
- `MODEL_LENGTH` 与 `MODEL_INCOMPLETE` 分别保留长度结束和未完成流分类。前端独立保存预算，不嵌套密钥表单，未保存连接编辑时禁止误改旧绑定。

本轮仅内部测试与源码集成，不构建新应用；没有原生或真实在线模型验收。
