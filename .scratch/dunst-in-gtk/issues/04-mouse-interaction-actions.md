# 04 — 鼠标交互与动作

**What to build:** 对齐 dunst 的鼠标语义：悬停暂停停留计时；左键点击触发默认动作（无动作则按 dismissed 关闭）；中键关闭（reason=2）；右键弹出动作菜单（无动作时菜单提供关闭项）；触发动作时向发起方发出 `ActionInvoked(us)` 信号，并正确处理动作键 `default` 与空动作键。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] 悬停时计时暂停，移开后恢复（集成测试：悬停 1.5s 超过 800ms 超时仍存活，移开后以 reason=1 关闭）
- [x] 带动作的通知左键点击 → `ActionInvoked(id, "default")`（配置 mouse_left_click = do_action）
- [x] 无动作时 do_action 关闭且 reason=2（中键测试路径覆盖：middle=[do_action, close_current]）
- [x] 中键点击 → 关闭且 reason=2
- [x] 右键弹出动作菜单（配置 mouse_right_click = context），点第二项 → `ActionInvoked(id, "other")`
- [x] 多个动作时菜单列出全部；`default` 键加 suggested-action 样式

## Comments

- 2026-08-13: 完成。架构决策：窗口只上报 `WindowEvent`（Closed/Hover/Click/Action），daemon 是唯一决策者（把 Click 映射到配置的 mouse_*_click 序列、发全部 D-Bus 信号）——为 ticket 05 状态机铺路。
- 可暂停计时器：glib SourceId + deadline；悬停 enter/leave 暂停/恢复。
- **发现并修复真实 bug**：GTK 信号回调嵌套——destroy 窗口时 motion controller 发 leave → with_daemon RefCell 双重借用 panic（abort）。改为 try_borrow_mut，嵌套事件丢弃。
- 默认鼠标绑定（dunst 语义）：left=close_current, middle=do_action,close_current, right=close_all；上下文菜单需配置 mouse_right_click=context 或经 dunstctl（ticket 06）。
- 集成测试要点：xdotool click 需要 mousemove 后 sleep ~0.2s；popover 是独立 X 窗口（带 1x1 输入兄弟窗口），按钮高度从 popover 几何反推。
