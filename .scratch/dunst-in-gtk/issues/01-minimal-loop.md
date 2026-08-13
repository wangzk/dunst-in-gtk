# 01 — 最小闭环：弹窗与关闭

**What to build:** 从零搭起守护进程骨架：持有 `org.freedesktop.Notifications` bus name（竞争失败则安静退出，退出码 0）；收到 `Notify` 调用后在屏幕角落弹出一个 GTK 通知窗口，显示标题与正文；`CloseNotification` 能关闭窗口并发出 `NotificationClosed(uu)` 信号；同时搭好集成测试基建（Xvfb + dbus-run-session + xdotool），用 `notify-send` 端到端验证"弹窗→关窗→收到信号"。

**Blocked by:** None — can start immediately

**Status:** ready-for-agent

- [x] `cargo run` 在无显示环境下给出清晰错误；在有 X 的环境下注册成功
- [x] 第二个 daemon 实例启动时检测到 bus name 已被占用，安静退出（退出码 0）
- [x] `notify-send "标题" "正文"` 弹出一个无装饰 GTK 窗口，显示标题与正文
- [x] `gdbus` 调用 `CloseNotification(id)` 后窗口消失，且调用方收到 `NotificationClosed(id, reason)` 信号
- [x] `GetCapabilities` / `GetServerInformation` 返回合法值
- [x] 集成测试脚本（Python/jeepney + xdotool，`tests/integration.py`）（dbus-run-session + Xvfb）可一键运行，断言窗口出现/消失与信号内容

## Comments

- 2026-08-13: 完成。实现中发现的 API 事实（均为环境核实）：
  - GTK4 移除了全部程序化窗口定位 API（`gtk_window_move`、`gdk_toplevel_move`、`GdkToplevelLayout` 位置字段），也移除了 `keep_above`/`skip_taskbar`/`type_hint`；gtk4-rs 0.11 连 `gdk::x11::X11Surface` 都删了。定位与 EWMH 提示（`_NET_WM_WINDOW_TYPE_NOTIFICATION`、`_NET_WM_STATE_ABOVE|SKIP_TASKBAR|SKIP_PAGER`）全部走 xcb 直连 X11（`src/x11.rs`）：realize 后按 `_NET_WM_PID`/标题找自己的 XID，在 GTK 的 map 请求之前 ConfigureWindow + ChangeProperty。
  - glib 0.22 移除了 `unix_signal_add` 和 `MainContext::channel`：信号用 signal-hook + glib 轮询；跨线程事件用 async-channel + `MainContext::spawn_local`。
  - zbus `request_name_with_flags` 对已占用名字返回 `Err(NameTaken)`（即使 DoNotQueue），据此安静退出（退出码 0）。
  - 集成测试用 Python + jeepney（纯 Python D-Bus 客户端）+ xdotool，替代 shell 方案（无转义地狱、可直接断言信号）。
