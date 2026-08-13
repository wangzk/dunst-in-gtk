# dunst-in-gtk spec

**Status:** ready-for-agent

## Problem Statement

dunst 直接用 X11 窗口 + cairo 手绘渲染通知，在 HiDPI 场景（高 DPI 显示器 + 缩放）下字体、图标、布局容易模糊或错位，每一处缩放都要手工处理。用户需要一个行为与 dunst 兼容（配置格式、控制接口、交互语义）但渲染交给 GTK 的通知守护进程，从而免费获得 GTK 的 HiDPI 支持：每显示器 scale factor 检测、设备像素渲染、图标主题按缩放比查找。

## Solution

Rust + GTK4 + zbus 的常驻通知守护进程 dunst-in-gtk：

- 每通知一个 GtkWindow（type hint NOTIFICATION + keep-above + 编程定位）
- 实现 freedesktop Notifications 规范（org.freedesktop.Notifications）
- 实现 org.dunstproject.cmd0 兼容控制接口，现有 dunstctl 零改动可用
- 读取 dunst 兼容的 dunstrc INI 配置子集（global + urgency 三段）
- 交互语义对齐 dunst：悬停暂停计时、左键默认动作、中键关闭、右键动作菜单
- 角落堆叠布局（gravity / gap / offset / 显示器选择）对齐 dunst geometry 语义

## User Stories

1. 作为 i3 用户，我想用 notify-send 发通知后屏幕上出现通知窗口，以便看到通知内容
2. 作为 i3 用户，我想通知显示在屏幕角落（默认右上角），以便不遮挡工作区
3. 作为 i3 用户，我想普通/低/高优先级通知有不同的默认停留时长，以便紧急通知更醒目
4. 作为 i3 用户，我想鼠标悬停在通知上时停留计时暂停，以便不因来不及读而错过内容
5. 作为 i3 用户，我想左键点击通知触发其默认动作，以便一键跳转到来源应用
6. 作为 i3 用户，我想中键点击关闭单条通知，以便手动清掉不想要的通知
7. 作为 i3 用户，我想右键点击弹出动作菜单选择非默认动作，以便在多个动作间选择
8. 作为 i3 用户，我想通知显示应用图标，以便快速识别来源
9. 作为 i3 用户，我想通知正文支持 HTML/markup（加粗、颜色），以便信息层次分明
10. 作为 i3 用户，我想带 value hint 的通知显示进度条，以便看到下载/安装进度
11. 作为 i3 用户，我想 dunstctl set-paused 开启免打扰，以便演示/录屏时不被打扰
12. 作为 i3 用户，我想 dunstctl history-pop 找回被关掉的通知，以便不错过重要消息
13. 作为 i3 用户，我想 dunstctl close / close-all / count / context / action 全部可用，以便沿用现有 i3 快捷键绑定
14. 作为多显示器用户，我想通知出现在鼠标所在显示器上，以便通知跟随当前工作位置
15. 作为多通知用户，我想多条通知按到达顺序在角落堆叠且互不重叠，以便同时查看多条
16. 作为音乐播放器用户，我想 replaces_id 通知替换同 ID 旧通知而不新开窗口，以便播放器状态条不刷屏
17. 作为开发者，我想关闭通知时收到 NotificationClosed 信号且关闭原因正确，以便客户端（libnotify 等）行为正确
18. 作为 dunst 迁移用户，我想直接复制现有 dunstrc 即可工作，以便迁移零成本
19. 作为高 DPI 用户，我想在 GDK_SCALE=2 下文字图标清晰且布局等比，以便视网膜屏体验正常
20. 作为 i3 用户，我想 daemon 随登录自动启动（systemd user unit），以便无需手动拉起
21. 作为 i3 用户，我想不同 urgency 的通知有不同的颜色/圆角/边框，以便视觉区分优先级
22. 作为 i3 用户，我想通知的宽度、高度上限、字体、对齐方式可配置，以便适配个人审美
23. 作为双 daemon 用户，我想 dunst-in-gtk 与 dunst 共存、手动切换，以便随时回退
24. 作为用户，我想 daemon 启动时若已有其他 daemon 持有 bus name 则安静退出，以便不制造双 daemon 混乱

## Implementation Decisions

- **技术栈**：Rust + gtk4-rs（GTK4）+ zbus 5 + serde；日志用 env_logger
- **窗口模型**：每通知一个 GtkWindow；GDK_WINDOW_TYPE_HINT_NOTIFICATION + keep-above + gtk_window_move 定位（X11 有效）；HiDPI 由 GTK4 每窗口 scale factor 自动处理，不自绘
- **渲染**：GtkBox 布局（图标 + 标题 + 正文）；Pango markup；GtkIconTheme 按图标名查找（含主题 fallback）；CSS 设置圆角/背景/前景/边框色/透明度；GtkProgressBar 渲染进度
- **配置**：手写 dunstrc INI 子集解析器（支持 `;`/`#` 注释、引号、大小写不敏感键）；global + urgency_low/normal/critical 三段；实现 L0+L1 相关键（font、geometry、gap_size、offset、corner_radius、background、foreground、frame_color、frame_width、timeout、urgency、icons、markup、word_wrap、alignment、progress_bar、history_length 等）；未知键警告并忽略
- **D-Bus A 面**（org.freedesktop.Notifications，well-known name 与路径均用规范值）：Notify(sssusasa{sv}i)→u、CloseNotification(u)、GetCapabilities()→as、GetServerInformation()→ssss；信号 NotificationClosed(uu)、ActionInvoked(us)
- **D-Bus B 面**（org.dunstproject.cmd0 接口，挂在 org.freedesktop.Notifications 同一 name/path 下，与 dunst 一致，dunstctl 直接可用）：方法 Ping、NotificationAction(u)、NotificationCloseLast、NotificationCloseAll、ContextMenuCall、NotificationClearHistory、NotificationShow、NotificationPopHistory(u)、NotificationRemoveFromHistory(u)、NotificationListHistory、ConfigReload(as)；属性 paused(b,rw)、pauseLevel(u,rw)、displayedLength(u)、historyLength(u)、waitingLength(u)；信号 NotificationHistoryCleared(u)、NotificationHistoryRemoved(u)、ConfigReloaded(as)。方法/属性签名以 dunst master 源码（src/dbus.c introspection XML + methods_dunst 表 + dunstctl 脚本）为准，已核实
- **状态机**：通知 ID 自增 u32；三态 waiting→displayed→closed；超时计时可暂停（悬停/免打扰）；replaces_id 同 ID 更新并重置计时；history 环形缓冲（容量 history_length）；免打扰 = pauseLevel>0 时通知进 waiting 不显示；关闭原因按规范枚举（1 expired / 2 dismissed / 3 close_notification / 4 undefined / 5 action_invoked）
- **布局**：dunst geometry 语义——WxH 为最大尺寸（0=自适应），X/Y 偏移 + gravity 九宫格定位；gap_size 通知间距；offset 屏幕边缘留白；monitor 选择支持编号与鼠标所在显示器（focus 模式不做）；每显示器独立堆叠
- **交互**：GtkEventControllerMotion 悬停暂停；左键=默认动作（无动作则按 dismissed 关闭）；中键=关闭；右键=GtkPopoverMenu 动作列表（无动作则显示关闭项）
- **生命周期**：bus name 竞争失败（已有 daemon）→ 正常退出码 0；SIGTERM/SIGINT 优雅退出；ConfigReload 重读配置并更新窗口
- **部署**：systemd user unit + i3 autostart desktop entry；与 dunst 共存，切换由用户手动执行
- **模块划分**：main（装配/生命周期）、config（INI 解析）、dbus（notifications + cmd0 两个接口）、daemon（状态机）、layout（几何数学）、window（GTK 窗口渲染交互）、icons（图标查找）

## Testing Decisions

- 原则：**只测外部可观察行为，不测 GTK 内部实现**。对 daemon 而言外部行为 = D-Bus 契约 + X11 窗口树
- **单元测试缝**：config 解析、layout 几何数学、daemon 状态机（注入虚拟时钟与事件，断言状态转移）——纯 Rust 模块，不依赖 GTK/D-Bus/显示，cargo test 直接跑
- **集成测试缝**：Xvfb + dbus-run-session 环境；驱动面 = org.freedesktop.Notifications + cmd0（用 notify-send / gdbus / dunstctl）；断言面 = NotificationClosed / ActionInvoked 信号 + cmd0 属性（displayedLength / waitingLength / historyLength）+ xdotool 查询窗口树几何；GDK_SCALE=1 与 =2 各跑一遍，断言窗口尺寸等比翻倍——HiDPI 正确性在此缝验证
- **冒烟**：用户 i3 真机会话手动验证（颜色、圆角、悬停、菜单、与 dunst 切换）

## Out of Scope

- L2 功能：rules 引擎、script/script_command 钩子、follow 模式的 focus 选项、image-data 图片通知、键盘快捷键
- dunstrc 全量键兼容（只实现 L0+L1 键，未知键警告忽略）
- Wayland 原生支持（layer-shell）与分数缩放（Wayland-only）
- 非 Linux 平台

## Further Notes

- 协议事实来源：dunst master 的 src/dbus.c（introspection XML、methods_dunst 表）、dunstctl 脚本、dunst.5 手册，已核实并记录
- 用户机器上现有 dunst 正在运行；切换方式：kill dunst → 启动 dunst-in-gtk，反向同理
- HiDPI 验证目标为 X11 整数缩放（GDK_SCALE），分数缩放依赖 Wayland，明确不承诺
- 项目名 dunst-in-gtk（用户指定）
