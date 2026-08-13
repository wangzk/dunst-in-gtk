# 08 — 打包：配置示例与命令行启动文档

**What to build:** 交付 `dunstrc.example`（带注释的默认配置，覆盖 L0+L1 全部键）、README（构建方式、命令行启动方式、与现有 dunst 的切换说明、dunstctl 兼容性说明）。明确**不做** systemd unit 与 autostart entry——daemon 由用户命令行直接启动。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] `dunstrc.example` 每个键有注释说明默认值与可选值
- [x] README 含：构建（cargo build --release）、启动（直接运行二进制，可带 `-config` 参数）、与 dunst 切换（kill dunst → 启动本程序，反向同理）
- [x] README 说明 dunstctl 兼容范围与已验证命令
- [x] 按 README 步骤从命令行启动可正常弹通知

## Comments

- 2026-08-13: 完成。dunstrc.example 覆盖全部已实现键（含新旧几何格式、urgency 段、L2 保留键）；README 含构建/启动/切换/dunstctl 兼容表/已知限制。冒烟：`-config dunstrc.example` 启动 + notify-send 弹窗成功。
- 顺带修复：GTK4 CSS 无 `max-width` 属性（进度条 CSS 曾触发 theme parser warning）——移除该规则，`progress_bar_max_width` 仅解析保留（dunst 兼容），进度条宽度受通知宽度自然约束。
- 顺带补测 ticket 03 遗留项：Xvfb 无法构造双屏（RandR < 1.5），单屏下覆盖 resolve_monitor 全部选择路径。

- 2026-08-13（GTK3 迁移）：渲染层从 GTK4 迁移到 GTK3——GTK4 移除了全部窗口 hint API（type_hint/accept_focus/keep_above/skip_taskbar/move），X11 后端在 map 时强制写入 WM_HINTS input=True，i3 下通知必然抢键盘焦点（xcb 直连无法对抗 GTK 的重写时序）；GTK3 保留全部官方 API，`set_accept_focus(false)` 一行解决。真机验证：三次通知焦点均留在终端，`_NET_WM_STATE` ABOVE/SKIP_TASKBAR/SKIP_PAGER 由官方 API 生效，x11.rs（250 行 xcb hack）整体删除。副作用：GTK3 的 `pixel-size` 只对 icon-name 生效，文件图标改为手动缩放（dunst 同款）。

- 2026-08-13（真机渲染修复）：GTK3 在真机 i3 上窗口内容全黑——三个根因（均用最小 C 程序/独立探针实证）：
  1. **widget 级 CSS provider 被系统 GTK3 3.24.52 静默忽略**（`style_context().add_provider` 无效；C 程序复现）→ 改用 `add_provider_for_screen`，每次通知添加新 provider（同优先级后添加者胜出，样式更新生效）
  2. **CSS 给 `window.notification` 节点设透明背景 → GTK3 跳过整窗重绘**（内容全黑）→ 窗口背景交给主题，box 承载背景/边框/圆角
  3. **padding 误用 widget margin**：box 缩进导致窗口边缘露出主题底色 → 改为 box 充满窗口 + CSS `padding`（背景/边框到边，内容内缩，与 dunst 一致）
  - 真机验证：半透明黑背景正确混合（#000000AA 在主题灰上 = 深灰 64,64,64），白色文字清晰可读，1px #AAAAAA 边框（用户配置，用户选择保留）
