# 飞书机器人 v1：最终仅 Node24 入口

已交付同协议Node worker、精确依赖锁、可拷贝运行资源、Rust bridge及离线证据。核心负责Keychain/IPC/公共持久inbox/最终应用资源集成。未连接真实飞书、未读取真实密钥、未使用computer-use。

原Python实现与 `feishu-bot.patch` 只作历史备份，不能合并为第二个运行入口。最终应用仅调用worker.mjs；请使用 `feishu-bot-node.patch`。

## 资源与启动

固定复用Pi现有 `./local-development-path`，实测v24.18.0。核心统一安排与Pi共用运行时，不使用PATH、不引入Python。数据库使用Node24内置node:sqlite，无额外SQLite npm包。

启动：`<固定Node24> --disable-warning=ExperimentalWarning <固定资源目录>/worker.mjs --state-dir <专用私有目录>`。参数只屏蔽Node内置SQLite实验性warning，不影响安全状态。目录来自可信后端，不接受前端/模型自由输入。

| 文件 | 处理 |
| --- | --- |
| worker.mjs / adapter.mjs | 唯一运行入口与范围/账本/HTTP策略，纳Git |
| package.json / package-lock.json | 精确锁定SDK及依赖，纳Git |
| node_modules/ | 完整运行依赖和许可证，拷贝为应用资源，不纳Git |
| runtime-resources.json | 完整856项资源SHA256及固定Node二进制SHA，仅本地清单 |
| resource-manifest.mjs | 重跑资源清单，不读配置/秘密，纳Git |
| bridge.rs | 同公开BotConfig/Inbound/BotWorker，启动参数已改Node，仅项目已有Rust依赖 |
| test-node.mjs | Node18项离线测试，含实际安装SDK的dispatcher/协议ACK |

已下载 `@larksuiteoapi/node-sdk@1.76.0`，52个锁定npm包，约40MB，全部resolved来自https://registry.npmjs.org/。仅在本任务副本以ignore-scripts安装，无生命周期脚本、全局安装、服务或旧CLI修改。SDK tarball integrity：`sha512-v/bxQNCugwAmMPbZtBGJtSu8hdQLiB1/29Jr+GBIG18DyQ4LYyp7gxQ74pkhXVSuoUeH4v+/ZHAw/SW+KMrj2g==`。应用启动时不npm安装。

资源清单排除Python、测试、缓存、配置、状态数据库。缺SDK/版本不符明确blocked/sdk_unavailable或sdk_version_unsupported，不回退CLI、不自动下载。

## 官方来源与传输边界

WS接收事件；消息发送独立使用HTTP POST /open-apis/im/v1/messages，只有message_id可确认发送成功。HTTP获取tenant_access_token，只在内存缓存，无用户身份OAuth。

- [官方Node SDK](https://github.com/larksuite/node-sdk)：另核对实际安装1.76.0的package/source、公开callbacks/getConnectionStatus/close和ACK路径。
- [事件订阅配置](https://open.feishu.cn/document/server-docs/event-subscription-guide/event-subscription-configure-/request-url-configuration-case)
- [接收消息及权限](https://open.feishu.cn/document/server-docs/im-v1/message/events/receive)
- [HTTP发送消息](https://open.feishu.cn/document/server-docs/im-v1/message/create)
- [自建应用token](https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal)

核实日期2026-10-16。SDK说明事件应尽快处理，平台超时可能重推；同应用多个客户端是分摊而非广播。核心只运行单例；接收回调只同步持久，不等待学习任务或模型。

## 范围和秘密

唯一固定appId+ownerOpenId+chatId。只收所选本人在所选聊天中的纯文本，其他人/聊天/机器人/图片/附件直接过滤且不持久正文。发送也固定到chatId。没有联系人/群列表/历史API，没有自动回复、任意URL、任意文件或代码执行、Pi工具调用。

平台订阅不是本地chatId服务端过滤器。独立自建app可用范围须只选本人、机器人只加入目标聊天；群聊只申请@机器人消息权限。已有广权限应用可能收到其范围内其他事件，不能默认为合适通道；本地仍严格本人AND聊天过滤。

secret仅来自用户未来在学习站安全配置入口自行填写，核心存独立Keychain项，启动时私有stdin传入。不读取旧CLI、旧verified配置、历史、环境secret或既有Keychain秘密。secret不可枚举，不写文件/SQLite/日志/前端持久化；SDK logger所有方法no-op，避免WS URL和正文落日志。不声称内存安全擦除。

## Protocol 1及确认

首行 `{"op":"init","config":{"appId":"cli_0123456789abcdef","ownerOpenId":"ou_…","chatId":"oc_…","receiveEnabled":false,"deliveryEnabled":false,"appSecret":"仅私有stdin"}}`。appId符合SDK的cli_加16位十六进制；收/发分别启用默认false；改绑定先停旧进程再明确保存，不接受重复init。

| 后续输入（实际严格JSON） | 行为 |
| --- | --- |
| `{op:"status"}` | 状态，不登录/发送 |
| `{op:"send",requestId,text}` | 显式发送，固定目标，需deliveryEnabled |
| `{op:"pending"}` | 重放最多100条未确认持久inbound |
| `{op:"ack",ticket}` | Rust事务成功后确认，sidecar清除正文 |
| `{op:"stop"}` / stdin EOF / SIGTERM | 停止退出，不自启 |

JSON行最多64KiB，命令字段封闭，队列最多128行。输出protocol/sequence/type；status含state/reason/counts/at，delivery含requestId/outcome，inbound含ticket/messageId/chatId/senderOpenId/text。原始协议含必要用户正文，不当日志或遥测。

Rust保留现有固定测试文字/请求号/outbox，调用worker前先预占Unknown。Node再次commit绑定+requestId账本才联网；同请求同正文返回原记录，改正文blocked。HTTP消息不重试，超时、丢回执、缺message_id或最终保存失败unknown；API明确拒绝failed，认证预检失败blocked。uuid由绑定+requestId确定，48字符。unknown先核对飞书实际消息，不自动换号发送。

接收先SQLite FULL提交，再输出并允许SDK协议ACK。实际SDK离线验证提交成功ACK200、回调存储失败ACK500。Rust以ticket/messageId唯一键写公共持久inbox后才ack；未确认条目重启pending重放。ack后sidecar正文NULL，只留去重标识。Rust队列100条、sidecar积压1000条；满时失败不报接收成功。长期离线不保证平台补齐，本实现不抓历史。

## 生命周期和核心接线

connecting→connected仅由SDK真实握手callback更新；onReconnecting先报disconnected再reconnecting，onReconnected报connected，onError报failed；每500ms读公开getConnectionStatus发现idle/failed。ready仅WS connected且进程活跃为true，不等于HTTP发送成功。Node无需Python私有方法override。

stop/EOF报stopping/stopped；SIGTERM中止HTTP并公开close({force:true})关闭WS、禁止SDK重连复活。Rust关stdin最多等2秒，超时kill+wait；在途发送Unknown预占保留。硬崩溃不保证最后stopped帧，桥接区分outputClosed和try_wait确认的processExited。退出/休眠不保证及时通知，不另加守护进程。

保留check_feishu_status/test_feishu_notification原IdentityStatus/DeliveryOutcome，改sender为同一Arc<BotWorker>，异步IPC用spawn_blocking。BotNotificationChannel复用原NotificationChannel；tick仍只落账，外部发送由核心显式启用和预占；Unknown映射非retryable的CHANNEL_UNKNOWN。核心补公共inbound持久入口后才ack，正文不注册任意执行工具。

核心负责安全配置、独立Keychain项、IPC、资源路径、单实例生命周期、公共inbox、最终Tauri资源配置。本任务未改共享Rust/src、未复制到共享目录、未提交。

## 用户将来最少操作

1. 自行创建仅学习站使用的企业自建应用，启用机器人，可用范围只选本人。App ID/Secret只填学习站安全设置，不发到此聊天。
2. 应用权限只申请im:message:send_as_bot及im:message.p2p_msg:readonly；明确选群才加im:message.group_at_msg:readonly并只加入目标群。不申请全群消息、历史、通讯录、附件或用户OAuth。自行完成组织发布/审批。
3. 事件与回调选择长连接，只加im.message.receive_v1。本人在目标聊天发送测试文字，从开发者控制台确认app专属ownerOpenId和chatId后明确保存；不扫描会话猜ID。控制台要求先连接时先在学习站手动连接再保存。
4. 手动启用接收验证本人消息，再单独启用发送点一次固定测试通知、核对message_id。unknown先查实际消息，不重复换号发。

## 验证边界

Node18项全过，含实际安装SDK事件解包和真实SDK帧ACK200/500、范围/去重/恢复/积压上限、Unknown、HTTP模拟、真实worker管道/SIGTERM/缺SDK。Rust2项过，桥接编译和实际Node私有管道。最小宿主结构按项目现有公开声明匹配，不代替最终完整应用编译。

仍缺真实凭据权限、握手/收发/断网重连/休眠，以及核心集成后的完整构建和原生验收。依赖准备好不等于用户已授权飞书应用；用户无需CLI或Python。
