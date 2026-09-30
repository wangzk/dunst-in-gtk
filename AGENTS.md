# AGENTS.md


## 编程规范

在使用GTK+3的API前先查阅GTK+3的官方文档，充分理解API的行为后再开始编程。不要主观臆断其行为。

## Agent skills

### Issue tracker
Issues and specs live as local markdown under `.scratch/<feature-slug>/`.
See `docs/agents/issue-tracker.md`.

### Triage labels
Default vocabulary — five canonical role labels, each equal to its name
(`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`).
See `docs/agents/triage-labels.md`.

### Domain docs
Single-context: root `CONTEXT.md` + `docs/adr/`. See `docs/agents/domain.md`.
