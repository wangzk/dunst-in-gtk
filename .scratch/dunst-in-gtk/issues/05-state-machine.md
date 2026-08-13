# 05 — 状态机细化：超时/替换/队列/免打扰

**What to build:** 完整通知状态机：urgency 默认超时与过期关闭（reason=expired）；`replaces_id` 同 ID 替换内容并重置计时而不新开窗口；displayed 上限之外的通知进 waiting 队列；`pauseLevel` 属性（读写）与 `paused` 属性，免打扰期间新通知进 waiting 不显示；cmd0 的 `displayedLength` / `waitingLength` / `pauseLevel` / `paused` 属性值始终正确；状态机纯逻辑可注入虚拟时钟单测。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] 默认超时到点后窗口消失且收到 reason=1 (expired)
- [ ] `replaces_id` 通知替换同 ID 窗口内容、不新增窗口、计时重置
- [ ] 通知数超过显示上限时多余通知排队，前面的关闭后自动补位
- [ ] `set-pause-level 1` 后新通知不显示、`waitingLength` 增加；恢复 0 后补显示
- [ ] `paused`/`pauseLevel` 属性经 `org.freedesktop.DBus.Properties` 读写正确，变更发属性变化信号
- [ ] 状态机单元测试覆盖：替换竞态、超时与免打扰叠加、排队补位边界
