# 08 — 打包：配置示例与命令行启动文档

**What to build:** 交付 `dunstrc.example`（带注释的默认配置，覆盖 L0+L1 全部键）、README（构建方式、命令行启动方式、与现有 dunst 的切换说明、dunstctl 兼容性说明）。明确**不做** systemd unit 与 autostart entry——daemon 由用户命令行直接启动。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] `dunstrc.example` 每个键有注释说明默认值与可选值
- [ ] README 含：构建（cargo build --release）、启动（直接运行二进制，可带 `-config` 参数）、与 dunst 切换（kill dunst → 启动本程序，反向同理）
- [ ] README 说明 dunstctl 兼容范围与已验证命令
- [ ] 按 README 步骤从命令行启动可正常弹通知
