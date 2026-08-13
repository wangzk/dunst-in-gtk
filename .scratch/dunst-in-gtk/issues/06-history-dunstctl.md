# 06 — history 与 dunstctl 全命令

**What to build:** history 环形缓冲（容量 history_length）：关闭的通知进 history；cmd0 的 `NotificationListHistory` 返回 JSON（dunst 同构）；`NotificationShow` / `NotificationPopHistory(u)` / `NotificationClearHistory` / `NotificationRemoveFromHistory(u)` 语义正确，配套 `NotificationHistoryCleared(u)` / `NotificationHistoryRemoved(u)` 信号；补齐 cmd0 其余方法 `Ping` / `NotificationAction(u)` / `NotificationCloseLast` / `NotificationCloseAll` / `ContextMenuCall` / `ConfigReload(as)`。验收直接用系统里的 dunstctl 脚本驱动。

**Blocked by:** 05

**Status:** ready-for-agent

- [x] `dunstctl history` 输出与 dunst 兼容（aa{sv} + busctl 转 JSON，字段含 id/appname/summary/body/urgency/timeout/timestamp，倒序）
- [x] `dunstctl history-pop` 重新显示最新历史通知（dunst 语义：保留在 history，用 history-rm 移除）
- [x] `history-pop <id>` / `history-rm <id>` / `history-clear` 行为正确，NotificationHistoryRemoved/Cleared 信号发出（集成测试断言）
- [x] `close`/`close-all`/`count`（displayed/waiting/historyLength）/`action`/`context`/`reload` 全部可用（真实 dunstctl 验证）；`ping` 是本机 dunstctl 版本没有的子命令，但 cmd0 Ping 方法已实现
- [x] `reload` 重读配置、重载窗口样式、更新队列上限（ConfigReload 实现）
- [x] history 环形缓冲（容量 history_length，去重按 id，超出挤最旧）

## Comments

- 2026-08-13: 完成。要点：
  - `NotificationListHistory` 返回 **aa{sv}**（不是 JSON 字符串！dunstctl 用 `busctl --json` 转 JSON）——字段对照 dunst 源码：id/appname/summary/body/icon_path/category/urgency/msg/timeout/timestamp/progress。
  - dunst 语义：`NotificationShow`/`NotificationPopHistory` 重新显示但**不移除** history 条目；`history-rm` 才移除。
  - history 与 dbus 线程共享：`Arc<Mutex<Vec<HistoryEntry>>>`（ListHistory/historyLength 直接读，其余操作走 GTK 线程事件）。
  - windows map 从 (window, urgency) 改为 (window, Pending)——reload 需要原始文本重载样式。
  - **测试基建重要发现**：jeepney 的 `filter()` 是客户端本地过滤，**不发总线 AddMatch**——广播信号（NotificationHistoryRemoved 等）收不到，定向信号（NotificationClosed）能收到。测试须先 `bus_proxy.AddMatch(rule)`。
