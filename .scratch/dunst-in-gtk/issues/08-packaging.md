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
