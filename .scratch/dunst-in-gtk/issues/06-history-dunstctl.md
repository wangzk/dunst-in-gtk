# 06 — history 与 dunstctl 全命令

**What to build:** history 环形缓冲（容量 history_length）：关闭的通知进 history；cmd0 的 `NotificationListHistory` 返回 JSON（dunst 同构）；`NotificationShow` / `NotificationPopHistory(u)` / `NotificationClearHistory` / `NotificationRemoveFromHistory(u)` 语义正确，配套 `NotificationHistoryCleared(u)` / `NotificationHistoryRemoved(u)` 信号；补齐 cmd0 其余方法 `Ping` / `NotificationAction(u)` / `NotificationCloseLast` / `NotificationCloseAll` / `ContextMenuCall` / `ConfigReload(as)`。验收直接用系统里的 dunstctl 脚本驱动。

**Blocked by:** 05

**Status:** ready-for-agent

- [ ] `dunstctl history` 输出 JSON 且结构与 dunst 兼容（含 id/appname/summary/body 等字段）
- [ ] `dunstctl history-pop` 把最新历史通知重新显示为窗口
- [ ] `dunstctl history-pop <id>` / `history-rm <id>` / `history-clear` 行为正确且信号发出
- [ ] `dunstctl close` 关闭最后一条、`close-all` 全关、`count` 三类数字正确、`action` 触发动作、`context` 打开菜单、`ping` 成功
- [ ] `dunstctl reload` 重读 dunstrc 并让新配置生效（窗口样式更新）
- [ ] history 容量上限：超出后最旧条目被挤出
