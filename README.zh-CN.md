# dunst-in-gtk

一个与 [dunst](https://dunst-project.org/) 兼容的桌面通知守护进程，用 GTK3 渲染通知窗口，用 Rust 实现。

- **协议兼容**：`org.freedesktop.Notifications`（Notify / CloseNotification / GetCapabilities / GetServerInformation）+ dunst 扩展接口 `org.dunstproject.cmd0`（dunstctl 直接可用）
- **配置兼容**：读取 dunst 的 `dunstrc`（新格式 `width/height/origin/offset` 与旧格式 `geometry` 都支持）
- **渲染**：GTK3 无装饰窗口、官方 EWMH 提示（`type_hint`/`keep_above`/`accept_focus` 等）、角落堆叠布局、图标（主题/文件）、Pango markup、进度条、动作菜单

## 构建

需要 Rust（≥1.75）与 GTK3 开发库（≥3.24，Ubuntu/Debian 长期维护）。

```bash
# Debian/Ubuntu
sudo apt install libgtk-3-dev
# Fedora
sudo dnf install gtk3-devel

cargo build --release
# 产物: target/release/dunst-in-gtk
```

## 启动

```bash
# 直接运行（无参数时按顺序查找配置：
#   $XDG_CONFIG_HOME/dunst/dunstrc → ~/.config/dunst/dunstrc）
target/release/dunst-in-gtk

# 指定配置文件
target/release/dunst-in-gtk -config /path/to/dunstrc

# 用法
dunst-in-gtk [-config <dunstrc>]
```

- 无需 systemd unit 或 autostart 条目：在会话启动时自行加入启动命令即可
- 若 `org.freedesktop.Notifications` bus name 已被占用（如有其他通知守护进程在运行），程序会安静退出（退出码 0）
- 没有配置文件也能运行（使用内置默认值）；完整键清单与默认值见 [`dunstrc.example`](dunstrc.example)

## 与现有 dunst 切换

```bash
# 停止 dunst
killall dunst

# 启动本程序（沿用你的 ~/.config/dunst/dunstrc）
target/release/dunst-in-gtk &

# 反向切换
killall dunst-in-gtk
dunst &
```

`notify-send "标题" "正文"` 端到端验证：

```bash
notify-send "hello" "world"
notify-send -u critical "重要" "紧急消息"
notify-send --hint int:value:30 "下载" "进行中…"        # 进度条
notify-send --hint string:image-path:/path/to/pic.png "图片"  # 文件图标
```

## dunstctl 兼容性

`org.dunstproject.cmd0` 接口已实现，系统中的 `dunstctl` 脚本可直接使用。已验证的命令：

| 命令 | 说明 |
|------|------|
| `dunstctl count` | displayed / waiting / history 数量 |
| `dunstctl history` | 历史通知列表（aa{sv}，经 busctl 转 JSON，字段与 dunst 一致） |
| `dunstctl history-pop [id]` | 重新显示最新（或指定）历史通知 |
| `dunstctl history-rm <id>` | 从历史移除 |
| `dunstctl history-clear` | 清空历史 |
| `dunstctl close` / `close-all` | 关闭最新 / 全部通知 |
| `dunstctl action [id]` | 触发通知动作 |
| `dunstctl context` | 弹出动作菜单 |
| `dunstctl is-paused` / `set-paused` | 免打扰状态读写 |
| `dunstctl reload` | 重读配置并应用到已显示的通知 |

未实现（本机 dunstctl 版本没有对应子命令，cmd0 方法也缺失）：`ping`、`debug`、`stack`、`rule`、`mouse`、`color`。

## 支持的 dunstrc 键

见 [`dunstrc.example`](dunstrc.example)（每个键都有注释、默认值与可选值）。覆盖：

- 布局：`width` / `height` / `origin` / `offset` / `gap_size`（及旧 `geometry`）
- 外观：`font` / `background` / `foreground` / `frame_color` / `corner_radius` / `frame_width` / `transparency` / `markup` / `word_wrap` / `ellipsize` / `alignment` / `vertical_alignment`
- 图标：`icons` / `icon_position` / `min_icon_size` / `max_icon_size` / `padding` / `horizontal_padding` / `text_icon_padding`
- 进度条：`progress_bar` / `progress_bar_height` / `progress_bar_frame_width` / `progress_bar_min_width` / `progress_bar_max_width`
- 行为：`history_length` / `notification_limit` / `monitor` / `follow` / `mouse_left_click` / `mouse_middle_click` / `mouse_right_click` / `timeout`（各 urgency 段）
- urgency 段 `[urgency_low]` / `[urgency_normal]` / `[urgency_critical]` 覆盖颜色与超时

未知键与 `[shortcuts]` / `[rules]` 等未实现段会产生警告日志，但不影响启动。

## 已知限制

- 仅 X11（Wayland 下 GTK3 窗口定位与 EWMH 提示不受支持；可通过 XWayland 运行）
- GTK3 已停止上游开发（Ubuntu 长期维护）；选择 GTK3 是因为其保留全部窗口 hint 官方 API（GTK4 已移除，导致通知在 i3 下会抢键盘焦点，且定位/置顶只能 xcb 直连）
- Xvfb 无 RandR 1.5，无法构造双屏；`monitor` 编号/名称/越界回退/`follow = mouse` 的选择路径已在单屏下集成验证，真机多屏请以配置为准
- `icon-data` / `icon_data` hint（内嵌图像数据）未实现；`image-path` / `image_path` 已支持

## 开发

```bash
# 单元测试（解析器 / 布局 / 队列 / 状态机 / 样式）
cargo test

# 集成测试（Xvfb + dbus-run-session + xdotool，端到端驱动 D-Bus）
tests/integration.py
```

集成测试覆盖：弹窗/关闭/信号、超时、队列与免打扰、replaces_id、角落堆叠与重排、HiDPI 几何、图标/markup/进度条（含 GDK_SCALE=2 像素断言）、鼠标交互、动作菜单、history、dunstctl 属性。

---

English version: [README.md](README.md)
