# 07 — 图标、markup 与进度条

**What to build:** 通知内容渲染补全：`app_icon` 与 `image-path` hint 经 GtkIconTheme 按名字查找（含主题 fallback 与缺失占位）；正文支持 Pango markup（加粗/颜色/链接，`body-markup` 能力声明）；`value` hint 渲染为进度条；icon 位置/大小随配置（icons 键）与 HiDPI scale 正确缩放。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] 带图标名的通知显示主题图标；图标名不存在时显示占位（应用名首字母或通用图标）
- [x] `<b>`/`<span color>` 等 markup 正确渲染；`markup` 配置为 no 时按纯文本显示
- [x] `value` hint (0-100) 显示进度条，随数值变化（replaces_id 更新）
- [x] `GetCapabilities` 正确声明 `body-markup`、`icon-static`、`actions` 等能力
- [x] GDK_SCALE=2 下图标像素尺寸翻倍（HiDPI 缝验证）

## Comments

- 2026-08-13: 完成。要点：
  - 图标全部走标准 GtkImage（`from_icon_name` / `from_pixbuf`）+ 逻辑 `pixel-size`，**零手动缩放**：GTK 的 scale factor 自动把 48 逻辑 px 渲染成 96 物理 px（GDK_SCALE=2），并按 `pixel_size × scale` 选择最佳主题资源（有 SVG 时 GTK 优先矢量）。
  - 关键事实（用独立探针 `GtkImage vs GtkPicture` 在 GDK_SCALE=1/2 下实测验证）：scale 只影响物理渲染、不影响逻辑布局；`pixel-size` 对 icon-name 与 pixbuf 模式都生效；GtkPicture 会被 Box 拉伸（halign Fill），不适合固定尺寸图标。
  - 集成测试的 HiDPI 断言用**相对比较**（scale2 橙色带高 ≈ 2× scale1，29px→55px）——firefox logo 的橙色带只占图标方块的一部分（48px 时 29px），不能用绝对像素断言。
  - dbus 接线对照 dunst 源码核实：`value` hint 接受 INT32/UINT32、负值=无进度条（history 存 -1）；`image-path`/`image_path` 覆盖 app_icon；`body-markup` 能力**条件声明**（`markup != no` 时，dunst 同款逻辑）。
  - 进度条样式：progress_bar_height/min_width/max_width 进 CSS；`progress_bar_frame_width` 用配置值（<0 时继承 frame_width）；`value` 经 replaces_id 原地更新。
  - markup=no 时字面量文本更宽（集成测试 55px vs 87px 窗口宽度差验证转义生效）。
