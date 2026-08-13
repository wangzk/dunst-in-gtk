# 05 — 状态机细化：超时/替换/队列/免打扰

**What to build:** 完整通知状态机：urgency 默认超时与过期关闭（reason=expired）；`replaces_id` 同 ID 替换内容并重置计时而不新开窗口；displayed 上限之外的通知进 waiting 队列；`pauseLevel` 属性（读写）与 `paused` 属性，免打扰期间新通知进 waiting 不显示；cmd0 的 `displayedLength` / `waitingLength` / `pauseLevel` / `paused` 属性值始终正确；状态机纯逻辑可注入虚拟时钟单测。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] 默认超时到点后窗口消失且收到 reason=1（expire_timeout 与 urgency 默认均生效，ticket 02/04 起）
- [x] `replaces_id` 同 ID 替换：不新增窗口（集成测试）、内容更新、计时重置（hover 中保持暂停）
- [x] 通知数超过 notification_limit 排队，关闭后 FIFO 补位（queue.rs 纯逻辑单测 + 集成测试 waitingLength/displayedLength）
- [x] pauseLevel>0 通知排队不显示、waitingLength 增加；恢复 0 补显（集成测试 + 真机 dunstctl set-paused 验证）
- [x] `paused`/`pauseLevel` 属性读写正确（zbus 属性名需显式 camelCase，如 `waitingLength`）；**真实 dunstctl is-paused/set-paused/count 零改动可用**
- [x] queue.rs 6 个纯逻辑单测：limit/免打扰/替换/移除/计数一致性

## Comments

- 2026-08-13: 完成。要点：
  - 队列逻辑抽为纯模块 `src/queue.rs`（无 GTK/D-Bus）：limit 上限、FIFO 等待、免打扰强制排队、关闭补位、replaces_id 原地更新。
  - 共享计数器 `DaemonCounters`（AtomicU32）：GTK 线程更新、dbus 线程（cmd0 属性）读取；queue 内部计数与计数器由 daemon 同步维护。
  - dunst 语义：`replaces_id > 0` 时通知 ID 直接用 replaces_id（不新分配）。
  - zbus 属性名默认 PascalCase（`DisplayedLength`），dunst 协议要 camelCase——用 `#[zbus(property, name = "waitingLength")]` 显式指定。
  - 替换显示中的通知时若指针悬停（hover），保持计时暂停。
  - **真实 dunstctl 验证通过**：count / is-paused / set-paused / 免打扰排队/补位（本机 dunstctl 直接驱动）。
