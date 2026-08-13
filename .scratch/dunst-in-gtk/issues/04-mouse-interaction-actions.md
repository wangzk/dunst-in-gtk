# 04 — 鼠标交互与动作

**What to build:** 对齐 dunst 的鼠标语义：悬停暂停停留计时；左键点击触发默认动作（无动作则按 dismissed 关闭）；中键关闭（reason=2）；右键弹出动作菜单（无动作时菜单提供关闭项）；触发动作时向发起方发出 `ActionInvoked(us)` 信号，并正确处理动作键 `default` 与空动作键。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] 悬停时计时暂停，移开后恢复（通过延长超时断言）
- [ ] 带动作的通知左键点击 → 调用方收到 `ActionInvoked(id, "default")`
- [ ] 无动作的通知左键点击 → 关闭且 reason=dismissed
- [ ] 中键点击 → 关闭且 reason=dismissed（dunst 语义）
- [ ] 右键弹出动作菜单，选择非默认动作 → `ActionInvoked(id, 动作键)`
- [ ] 多个动作时菜单列出全部；`default` 键在菜单中正确标注
