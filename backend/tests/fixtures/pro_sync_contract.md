# PRO 目标 JSON 恢复协议

`pro_sync_contract.json` 摘自实际 Emby 4.10.2.0、Strm Assistant PRO 3.0.0.57 的 `/swagger.json`，保留目标同步操作及 Bundle 模型。最初依据 DLL 推断空数组会读取正式 JSON；后续实际调用否定了该推断。

调用必须为管理员 `POST /Items/SyncMediaInfo?Id=原生媒体ID`，正文是准备完成的完整非空单 Bundle 数组，插件处于 Restore 模式且 `IsModSupported=true`。使用明确的单个 Id，不传 Path 或 Ids，也不调用 ClearMediaInfo。普通 `/Items/{Id}/Refresh` 的 204 和完成日志不表示 JSON 被加载；61616 的原失败日志中该刷新只执行了 11 ms，没有反序列化。

实际 `[]` 请求返回 HTTP 200 与 `[]`，没有导入流或章节；`[{}]` 也没有导入，`[null]` 触发异常。测试不得把这些请求模拟成隐式读取正式 JSON。

核对的 PRO DLL SHA-256：`1053d3d6b5d6923ebbda797ca9ca549a9294b68861a78fba30bda9a52150c94e`。

- handler `0x0600016a` 用非空 Id 查找目标。传入完整非空 Bundle 后，SDK 会过滤 IntroStart、IntroEnd、CreditsStart 章节，以及本地外挂 Audio/Subtitle 流（IsExternal 且 Protocol 为 File；缺省与数值 0 同为本地协议）。
- apply `0x06001a6b` 按目标 InternalId 覆盖已有媒体流与章节，无须先清空。同步前必须检查准备正文和当前原生数据；二者任一含上述被过滤内容时停止，避免覆盖丢失已有内容。多 Bundle 同样提前拒绝；多个目标必须分别以各自 Id 和单 Bundle 请求同步。
- 正式文件的完整内容就是准备正文。SDK 模拟必须应用收到的正文，不能靠再次读取正式文件制造成功。
- 章节路径 `0x06001a57` 用目标本地 BIF 调用 RefreshThumbnailImages，`extractImages=false`。同步保留普通章节和已有 BIF，图片标签与 BIF 仍需 API 回读验收。
- Audio 的 EmbeddedInfo 会覆盖现有名称、简介、标签等，EmbeddedImage 也可能覆盖或额外创建主图。导入正文必须排除这两个字段，无论目标目前是否已有主图；确认同步返回且回读成功后，正式 JSON 与缓存保留原完整内容。未确认时保留完整备份和恢复说明。

61616 的一次完整非空 Movie Bundle 实际同步返回 HTTP 200，并验收通过：1 条 Video、4 条 Audio、11 条 Subtitle、14 个普通章节、14 个章节图片标签、5079 个 BIF 缩略图；名称、简介、流派、人物、ImageTags、ProviderIds 的前后校验相同。该结果证明普通内容可以直接覆盖原生信息，不证明被 SDK 过滤的内容能够安全恢复。

这些语义仅针对已核对的 3.0.0.57 与可用 mod。版本、接口、单 Bundle 或原生数据保护检查不符合时，在同步前停止，也不退回普通刷新或全库提取任务。自动化回归全部使用隔离的 localhost 上游。
