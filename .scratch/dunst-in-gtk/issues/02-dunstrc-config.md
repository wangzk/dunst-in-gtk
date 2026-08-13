# 02 — dunstrc 配置生效

**What to build:** 手写 dunstrc INI 子集解析器（`;`/`#` 注释、引号、大小写不敏感键，global + urgency_low/normal/critical 三段），把颜色、圆角、边框、字体、默认超时等应用到通知窗口；未知键警告并忽略；解析器有完整单元测试。用户复制现有 dunstrc 即可迁移。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] 解析 `global` 段与三个 urgency 段的 L0+L1 键（font、background/foreground/frame_color、corner_radius、frame_width、timeout、icons、markup、word_wrap、alignment 等）
- [x] 注释/引号/键名大小写/段名大小写（`urgency_low` 与 `urgency_LOW` 等价）处理正确
- [x] 未知键产生警告日志但不中断启动；缺省键使用文档化的默认值
- [x] 窗口背景/前景/边框/圆角/字体随配置变化（style_css 单测 + 用户真实 dunstrc 冒烟）（集成测试截图或 CSS 属性断言）
- [x] 单元测试覆盖解析边界（缺文件、空段、重复键、畸形行）

## Comments

- 2026-08-13: 完成。重要事实：本机 dunst 1.13.2 + 用户真实 dunstrc 已是**新配置格式**（`width=(min,max)`/`height`/`origin`/`offset`，取代旧 `geometry`），因此解析器以新格式为主、兼容旧 `geometry = WxH+X+Y`（含 `offset = NxN`）。`frame_color` 是 global 段合法键（urgency 覆盖，dunst 语义）。用户配置里 `word_wrap = yes`（bool 接受 yes/no）。`scale` 键解析但忽略（GTK 处理 HiDPI）。`[experimental] per_monitor_dpi` 属 L2，警告跳过。
- GTK4 的 GtkLabel 移除了 `font_desc` 属性：字体用 Pango `AttrFontDesc` 属性列表设置。
- 真实配置冒烟：仅 2 条良性警告，窗口正常弹出（位置/宽度约束属于 ticket 03）。
