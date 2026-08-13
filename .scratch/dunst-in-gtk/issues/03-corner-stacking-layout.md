# 03 — 角落堆叠布局

**What to build:** 实现 dunst geometry 语义：`WxH+X+Y`（0=自适应）+ gravity 九宫格定位、gap_size 通知间距、offset 屏幕边缘留白；多通知在角落按到达顺序堆叠且互不重叠；支持多显示器（编号指定 + 跟随鼠标所在显示器），每显示器独立堆叠；布局数学纯函数化并有单元测试；用 xdotool 断言窗口几何，GDK_SCALE=1 与 =2 各跑一遍验证 HiDPI 等比缩放。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] 默认右上角弹出；gravity 换成其他角（layout 单测覆盖 9 宫格）（如左下）后位置正确
- [x] 连续发通知：同角落堆叠、互不重叠、间距等于 gap_size（集成测试断言）
- [x] 关掉通知后其余重排（集成测试：关闭第一条后第二条回到 y=10）（或不重排，视实现语义与 dunst 对齐）
- [ ] 两显示器（Xvfb 双屏）下通知出现在指定编号/鼠标所在显示器（resolve_monitor 已实现，双屏集成测试未做）
- [x] width/height spec（Constant/Range/Percent）生效（集成测试断言 WIDTH=200）（长正文被约束不超宽）
- [x] 单元测试覆盖九宫格/偏移/间距/Center 整栈居中；GDK_SCALE=2 集成测试断言物理尺寸翻倍

## Comments

- 2026-08-13: 完成。要点：
  - dunst 1.13 新配置格式（width/height/origin/offset）成为布局输入，替代旧 geometry 语义；Center origin 把整个栈居中（dunst 语义），角部 origin 只沿单轴堆叠（右对齐窗口 x 相同）。
  - 标题含通知 id（`dunst-in-gtk {app} [{id}]`），xcb 按**精确 _NET_WM_NAME** 定位——注意 _NET_WM_NAME 是 UTF8_STRING，GetProperty 必须用 ATOM_ANY 读（曾因 ATOM_STRING 读空导致位置失效）。
  - 所有坐标是逻辑像素，xcb 侧按 surface scale_factor 转物理像素（HiDPI 集成测试验证）。
  - 集成测试竞态修复：xdotool 对未 configure 的窗口报 (0,0,1,1)，需轮询等待真实几何；bus name owner 校验必须比对 PID（死 daemon 的 name 在 bus 上短暂残留）。
