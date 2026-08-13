# 02 — dunstrc 配置生效

**What to build:** 手写 dunstrc INI 子集解析器（`;`/`#` 注释、引号、大小写不敏感键，global + urgency_low/normal/critical 三段），把颜色、圆角、边框、字体、默认超时等应用到通知窗口；未知键警告并忽略；解析器有完整单元测试。用户复制现有 dunstrc 即可迁移。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] 解析 `global` 段与三个 urgency 段的 L0+L1 键（font、background/foreground/frame_color、corner_radius、frame_width、timeout、icons、markup、word_wrap、alignment 等）
- [ ] 注释/引号/键名大小写/段名大小写（`urgency_low` 与 `urgency_LOW` 等价）处理正确
- [ ] 未知键产生警告日志但不中断启动；缺省键使用文档化的默认值
- [ ] 窗口背景/前景/边框/圆角/字体随配置变化（集成测试截图或 CSS 属性断言）
- [ ] 单元测试覆盖解析边界（缺文件、空段、重复键、畸形行）
