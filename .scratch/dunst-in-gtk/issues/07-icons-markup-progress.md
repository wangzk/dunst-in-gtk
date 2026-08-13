# 07 — 图标、markup 与进度条

**What to build:** 通知内容渲染补全：`app_icon` 与 `image-path` hint 经 GtkIconTheme 按名字查找（含主题 fallback 与缺失占位）；正文支持 Pango markup（加粗/颜色/链接，`body-markup` 能力声明）；`value` hint 渲染为进度条；icon 位置/大小随配置（icons 键）与 HiDPI scale 正确缩放。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] 带图标名的通知显示主题图标；图标名不存在时显示占位（应用名首字母或通用图标）
- [ ] `<b>`/`<span color>` 等 markup 正确渲染；`markup` 配置为 no 时按纯文本显示
- [ ] `value` hint (0-100) 显示进度条，随数值变化（replaces_id 更新）
- [ ] `GetCapabilities` 正确声明 `body-markup`、`icon-static`、`actions` 等能力
- [ ] GDK_SCALE=2 下图标像素尺寸翻倍（HiDPI 缝验证）
