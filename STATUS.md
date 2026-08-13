# dunst-in-gtk — 工作交接状态（2026-08-13 暂停时记录）

> 本文件是暂停时的快照，恢复工作时先读它，再读 `.scratch/dunst-in-gtk/` 下的
> spec 与 tickets。**最后一条 git 提交：`61d584c`（ticket 02）**。

## 项目一句话

用 Rust + GTK4 重写 dunst 的 Linux 通知守护进程：每通知一个 GtkWindow，
D-Bus 协议（org.freedesktop.Notifications + org.dunstproject.cmd0）与
dunstrc 配置与 dunst 兼容，借 GTK 获得 HiDPI 支持。

## 当前进度

| Ticket | 状态 | 说明 |
|---|---|---|
| 01 最小闭环 | ✅ 完成（d0f2a16） | D-Bus Notify/Close + GTK 弹窗 + X11 EWMH + 集成测试 |
| 02 dunstrc 配置 | ✅ 完成（61d584c） | 新格式为主 + 旧 geometry 兼容；用户真实配置可加载 |
| 03 角落堆叠布局 | 🔶 **进行中（未提交）** | 代码已写完，单测 23 个全绿；集成测试有 1 个已知竞态未修 |
| 04 鼠标交互与动作 | ⬜ 未开始 | |
| 05 状态机（超时/替换/队列/DnD） | ⬜ 未开始 | |
| 06 history + dunstctl 全命令 | ⬜ 未开始 | 依赖 05 |
| 07 图标/markup/进度条 | ⬜ 未开始 | |
| 08 打包（dunstrc.example + README） | ⬜ 未开始 | 明确**不做** systemd/autostart，命令行启动 |

## 工作区未提交的改动（ticket 03 全部）

- `src/layout.rs`（新增）：纯布局数学 — `resolve_size`（Constant/Range/Percent）、
  `stack_position`（九宫格 origin + offset + gap，Center origin 整体居中）。
  23 个单测含 layout 的 8 个，全绿。
- `src/window.rs`：WindowStyle/style_css/render_text；标题改为
  `"dunst-in-gtk {app} [{id}]"`（id 唯一化，供 X11 精确定位）；
  `natural_size`/`height_for_width`/`apply_geometry`（首调 realize+hints+present，
  后续只发 xcb configure）。
- `src/x11.rs`：定位+EWMH 合一；按**精确 _NET_WM_NAME** 找 XID（之前按
  PID+前缀会误伤同 app 的兄弟窗口）；坐标按 surface scale factor 换算物理像素。
- `src/daemon.rs`：`relayout()` 编排（排序 urgency desc → 尺寸解析 → 逐窗定位）；
  `resolve_monitor`（follow mouse/编号/名称）。
- `src/config.rs`：`Monitor::Name(String)`；`frame_color` 为 global 合法键
  （urgency 覆盖）。
- `src/main.rs`：`mod layout`；`-config` 参数。
- `tests/integration.py`：新增 `test_layout`（堆叠/间距/关闭重排断言）、
  `test_hidpi`（GDK_SCALE=2 物理尺寸翻倍断言）、`test_name_conflict` 改为自起
  持有者；`Daemon.start` 支持 args/env。

## 恢复工作时的下一步（按顺序）

1. **修 ticket 03 集成测试竞态**（唯一已知失败）：
   `FAIL: expected top-right at x=1070 w=200 ..., got [(0, 0, 1, 1)]`
   原因：`wait_window_count` 检测到窗口（realize 后即有 XID/标题）后立即查几何，
   xdotool 读到未 configure 的 1x1。修法：加 `wait_window_geometry` 辅助函数，
   轮询直到几何非 (0,0,1,1) 再断言。
2. 跑 `python3 tests/integration.py` 全绿后，提交 ticket 03（含更新
   `.scratch/dunst-in-gtk/issues/03-corner-stacking-layout.md` 验收项 + Comments）。
3. 继续 frontier：**ticket 04（鼠标交互与动作）**——配置里已解析
   `mouse_left/middle/right_click`；需要 GtkEventControllerMotion 悬停暂停、
   左键默认动作→ActionInvoked、中键关闭、右键 GtkPopoverMenu；actions 需从
   Notify 传入（当前 `_actions` 被忽略）。

## 关键技术事实（已踩坑，勿重查）

- **GTK4 移除了全部程序化窗口定位 API**（gtk_window_move / gdk_toplevel_move /
  GdkToplevelLayout 位置字段），也移除了 keep_above/skip_taskbar/type_hint；
  gtk4-rs 0.11 连 `gdk::x11::X11Surface` 都没有。→ 定位与 EWMH 全走 xcb 直连
  X11（`src/x11.rs`），realize 后按唯一标题找 XID，在 GTK map 前 ConfigureWindow
  + ChangeProperty；坐标 × surface.scale_factor() 转物理像素。
- glib 0.22 移除了 `unix_signal_add`（用 signal-hook + glib 轮询）和
  `MainContext::channel`（用 async-channel + `MainContext::spawn_local`）。
- zbus `request_name_with_flags` 对已占用名字返回 `Err(zbus::Error::NameTaken)`
  （即使 DoNotQueue），据此退出码 0。
- zbus `blocking::Connection` 自带内部 executor 线程，方法自动分发；
  `blocking_emit_signal` 在 blocking Connection 上叫 `emit_signal`。
- zbus 接口方法取客户端唯一名：`#[zbus(header)] hdr: zbus::message::Header`，
  `hdr.sender()`；Notify 签名 `susssasa{sv}i`（replaces_id 是第 2 个字段）。
- GDK4 X11 每个通知窗口会伴随一个无名的兄弟窗口（组窗口），无标题，无害。
- dunst 1.13+ 配置是**新格式**（width/height/origin/offset），旧 geometry 兼容；
  `frame_color` 是 global 键；bool 接受 yes/no；颜色 #RGB/#RGBA/#RRGGBB/#RRGGBBAA。
- 集成测试环境：Xvfb + dbus-run-session + **jeepney**（纯 python D-Bus 客户端，
  已 pip 安装）+ xdotool。测试内起多个 daemon 时注意 bus name 竞争
  （谁持有 org.freedesktop.Notifications 谁接 Notify）。

## 验证命令

```bash
cargo test                 # 单测（当前 23 个全绿）
python3 tests/integration.py   # 集成（当前 1 个已知竞态待修）
```

## 备注

- `.pi/`（pi 任务文件）已加入 .gitignore，勿提交。
- 依赖：gtk4 0.11、glib 0.22、zbus 5、async-channel 2、xcb 1、signal-hook 0.3、
  env_logger。jeepney 是测试期 python 依赖。
- 用户在真机 i3 + X11 上运行 dunst 1.13.2，真实配置 `~/.config/dunst/dunstrc`
  已用作兼容性冒烟目标。
