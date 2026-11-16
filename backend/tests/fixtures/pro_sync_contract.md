# PRO 目标 JSON 恢复协议

`pro_sync_contract.json` 摘自实际 Emby 4.10.2.0、Strm Assistant PRO 3.0.0.57 的 `/swagger.json`，保留目标同步操作及 Bundle 模型。最初依据 DLL 推断空数组会读取正式 JSON；后续实际调用否定了该推断。

调用必须为管理员 `POST /Items/SyncMediaInfo?Id=原生媒体ID`，正文是准备完成的完整非空单 Bundle 数组，插件处于 Restore 模式且 `IsModSupported=true`。使用明确的单个 Id，不传 Path 或 Ids。HTTP 200、普通 `/Items/{Id}/Refresh` 的 204 和完成日志都不单独证明 JSON 已导入，仍需核对实际媒体信息。

实际 `[]` 请求返回 HTTP 200 与 `[]`，没有导入流或章节；`[{}]` 也没有导入，`[null]` 触发异常。测试不得把这些请求模拟成隐式读取正式 JSON。

核对的 PRO DLL SHA-256：`1053d3d6b5d6923ebbda797ca9ca549a9294b68861a78fba30bda9a52150c94e`。

- handler `0x0600016a` 用非空 Id 查找目标。传入完整非空 Bundle 后，SDK 会过滤 IntroStart、IntroEnd、CreditsStart 章节，以及本地外挂 Audio/Subtitle 流（IsExternal 且 Protocol 为 File；缺省与数值 0 同为本地协议）。
- 目标已有完整原生媒体信息时，导入器可能提前跳过正文并返回当前数据。非空 Bundle 也不能证明已有字段被覆盖；测试需保留这种行为。
- 正式文件的完整内容就是准备正文。SDK 模拟对可导入的目标应用收到的正文，不能靠再次读取正式文件制造成功。多 Bundle 提前拒绝；多个目标分别以各自 Id 和单 Bundle 请求同步。
- 章节路径 `0x06001a57` 用目标本地 BIF 调用 RefreshThumbnailImages，`extractImages=false`。同步保留普通章节和已有 BIF，图片标签与 BIF 仍需 API 回读验收。
- Audio 的 EmbeddedInfo 会覆盖现有名称、简介、标签等，EmbeddedImage 也可能覆盖或额外创建主图。导入正文必须排除这两个字段，无论目标目前是否已有主图；确认同步返回且回读成功后，正式 JSON 与缓存保留原完整内容。未确认时保留完整备份和恢复说明。

已有完整媒体未导入编辑值时，只有确认清理范围是唯一当前媒体、没有关联版本或附加内容，并保护原 JSON、BIF 和章节图片后，才允许定点清理与恢复。无法确认范围时，媒体编辑保留原材料；正常入库和重新整理继续按原流程更新所选媒体的元数据。

已有片头片尾标记不能通过会过滤标记的同步请求恢复；在已验证的神医配置和唯一媒体范围内，使用 `Default + ValidationOnly` 文件恢复，并核对原标记、媒体信息和图片。普通 FullRefresh 不作为 JSON 导入的替代。

这些恢复语义仅针对已核对的 3.0.0.57 与可用 mod。自动化回归全部使用隔离的 localhost 上游，版本、接口和恢复保护检查不构成普通入库的技术字段或非空标题准入条件。
