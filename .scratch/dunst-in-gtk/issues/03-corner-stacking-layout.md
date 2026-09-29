# 03 — 角落堆叠布局

**What to build:** 实现 dunst geometry 语义：`WxH+X+Y`（0=自适应）+ gravity 九宫格定位、gap_size 通知间距、offset 屏幕边缘留白；多通知在角落按到达顺序堆叠且互不重叠；支持多显示器（编号指定 + 跟随鼠标所在显示器），每显示器独立堆叠；布局数学纯函数化并有单元测试；用 xdotool 断言窗口几何，GDK_SCALE=1 与 =2 各跑一遍验证 HiDPI 等比缩放。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] 默认右上角弹出；gravity 换成其他角（layout 单测覆盖 9 宫格）（如左下）后位置正确
- [x] 连续发通知：同角落堆叠、互不重叠、间距等于 gap_size（集成测试断言）
- [x] 关掉通知后其余重排（集成测试：关闭第一条后第二条回到 y=10）（或不重排，视实现语义与 dunst 对齐）
- [x] 两显示器下通知出现在指定编号/鼠标所在显示器（resolve_monitor 编号/名称/越界回退/follow=mouse 路径单屏集成验证；Xvfb 无 RandR 1.5 无法构造双屏，真机多屏待用户验证）
- [x] width/height spec（Constant/Range/Percent）生效（集成测试断言 WIDTH=200）（长正文被约束不超宽）
- [x] 单元测试覆盖九宫格/偏移/间距/Center 整栈居中；GDK_SCALE=2 集成测试断言物理尺寸翻倍

## Comments

- 2026-08-13: 完成。要点：
  - dunst 1.13 新配置格式（width/height/origin/offset）成为布局输入，替代旧 geometry 语义；Center origin 把整个栈居中（dunst 语义），角部 origin 只沿单轴堆叠（右对齐窗口 x 相同）。
  - 标题含通知 id（`dunst-in-gtk {app} [{id}]`），xcb 按**精确 _NET_WM_NAME** 定位——注意 _NET_WM_NAME 是 UTF8_STRING，GetProperty 必须用 ATOM_ANY 读（曾因 ATOM_STRING 读空导致位置失效）。
  - 所有坐标是逻辑像素，xcb 侧按 surface scale_factor 转物理像素（HiDPI 集成测试验证）。
  - 集成测试竞态修复：xdotool 对未 configure 的窗口报 (0,0,1,1)，需轮询等待真实几何；bus name owner 校验必须比对 PID（死 daemon 的 name 在 bus 上短暂残留）。

- 2026-08-13: 补测（ticket 08 顺带收尾）。Xvfb 的 RANDR 低于 1.5（xrandr --setmonitor 静默无效），无法生成双 monitor；改为单屏下覆盖 resolve_monitor 全部选择路径（monitor=0 / 越界 99 回退 / monitor=screen 名称匹配 / follow=mouse），断言窗口落位 top-left(10,10)。真机双屏留给用户。

- 2026-09-29: 真机复查（i3 + X11；DP-0 竖屏 2160x3840 在左、DP-2 主屏 3840x2160 在右，虚拟屏 6000x3840）。用户报"通知横跨两块屏、不在单屏中间"，定位到 4 个真实缺陷并修复：

  1. **`follow = mouse` 从未生效**：`gdk_device_get_window_at_position()` 对 `default_seat().pointer()` 给出的虚拟/主设备（"Virtual core pointer"）返回 NULL，代码于是走回退分支 `monitors[0]`（本机 GDK 索引 0 = DP-2），通知永远落在主屏。改为 `gdk_device_get_position()` 取坐标 + `gdk_display_get_monitor_at_point()`；回退也从"GDK 索引 0"改成"primary"（索引顺序后端相关，不保证 0 是主屏）。真机验证：指针 (1000,1000)→DP-0、(4000,1000)→DP-2。

  2. **窗口被 WM 搬到焦点工作区**：i3 `manage_window()` 用 `xcb_get_geometry` 的实际坐标建浮动窗口，随后 `floating_enable()` 判断窗口是否在**焦点工作区所在输出**上，不在则 `floating_fix_coordinates()` 按相对位置搬过去——实测请求 x=830 落到 3830，正好等于 `2160 + 0.5*3840 - 250`。另外被管理的窗口还会被 i3 加上边框/标题栏，几何固定偏移 (+4, +44)。修复：映射前把窗口设成 **override-redirect**（GTK3 的 GtkMenu/tooltip 亦然；i3 `manage_window()` 明确跳过 OR 窗口），WM 从此不参与定位；非 X11 后端跳过该调用（后端判定必须用 GType 名 `GdkX11Display`——`gdk_display_get_name()` 返回的是 display 字符串 ":0"，据此判断会永远为假、OR 静默失效，这个坑踩过一次）。

  3. **自然尺寸超过 width spec → 窗口变宽跨界**：GTK 对非 resizable 顶层窗口取 `max(default_size, natural_size)`，而 summary 标签原先没有 wrap/ellipsize，长单行标题把自然宽度撑到 2160（整块竖屏宽），布局按 1000 算的居中坐标与实际窗口不符 → 横跨两屏。修复：summary 与 body 一致设置 wrap/ellipsize；再用 `set_max_width_chars` 显式钳制自然宽度（实测 default_size/size_request/geometry hints 都会被自然尺寸覆盖，只有 max_width_chars 有效），并按测量值迭代收敛（字符→像素换算要用**配置字体**的 metrics，不是主题字体）。

  4. **relayout 之前测量恒为 0**：`preferred_size()` 在控件 visible 前返回 0x0，relayout 一直按 1x1 计算，窗口尺寸实际由 GTK 自然尺寸决定（历史行为只是"碰巧"看起来对）。修复：构造时对内容 `show_all()`（顶层仍保持隐藏，映射时才定位），`apply_geometry` 对已显示窗口调用 `resize()`（`set_default_size` 只对未映射窗口生效）。

  真机验证方式（不依赖任何长期守护进程）：自建临时进程 + 私有 D-Bus + 唯一标题 marker，测完 `kill -9` 并确认无残留窗口。结果：竖屏 top-center + offset(0,100) 下，宽 1000 → x=580、宽 500 → x=830；主屏 x=3830 = 2160+(3840-500)/2；双条通知 y=100/220（gap 0、h=120）；映射 400ms 后位置不变。

  集成测试同步改为**与显示器几何自洽**（坐标断言从 `xrandr` 推导、像素扫描按窗口实际位置裁剪），因此现在可直接在真实多屏会话运行：`DISPLAY=:0 dbus-run-session -- python3 tests/integration.py target/release/dunst-in-gtk --inside`（沙箱内 Xvfb 因 /tmp/.X11-unix 权限起不来，属环境限制）。


- 2026-09-29（续）真机套件全绿：`DISPLAY=:0 dbus-run-session -- python3 tests/integration.py target/release/dunst-in-gtk --inside` → 18 个用例全部 PASS（含悬停暂停、左/中/右键、右键菜单键盘导航、replaces_id、队列上限、DND、history、名称冲突、SIGTERM），验证 override-redirect 未破坏输入/绘制/菜单路径。为在真机可运行而做的测试改动（均为环境无关化，断言强度不降）：
  - 坐标断言从 xrandr 实际显示器几何推导（`xrandr_monitors` / `monitor_with_right_edge` / `monitor_at_origin`）；monitor 名称用例改为"配置里钉住 xrandr 报出的连接器名 + 断言该显示器"。
  - 像素扫描改为按窗口实际几何裁剪（`window_box`），避免读到桌面壁纸。
  - 修正 monitor 选择用例的配置模板 bug（`monitor = {monitor}` 与用例值里的 `monitor = ` 叠加成 `monitor = monitor = 0`，名称路径此前从未真正生效）。
  - 沙箱内 Xvfb 无法启动（`/tmp/.X11-unix` 不存在且非 root 无法创建/改属主），因此 `--inside` 直连 `:0` 是当前唯一可跑的集成测试路径；产品代码不依赖这一点。
