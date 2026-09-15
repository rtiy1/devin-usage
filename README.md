# devin-usage

Devin CLI 本地用量监控小工具（Tauri 桌面应用）。

- 实时仪表盘：每 2 秒从本地 `sessions.db` 重算 token 用量
- 配额面板：日配额 / 周配额进度条 + 重置倒计时（读取 Devin CLI 的 user_status 缓存）
- 今天 / 本周 / 总计，按模型、按天、按会话拆分
- 全部本地只读，不联网、不需要登录态

## 数据源

| 数据 | 位置 |
| --- | --- |
| token 用量 | `%APPDATA%\devin\cli\sessions.db` |
| 配额状态 | `%LOCALAPPDATA%\devin\cli\user_status.*.bin`（CLI 运行时刷新） |

## 构建

```bash
npm ci
npx tauri build
```

产物：`src-tauri/target/release/devin-usage.exe`（单文件，可直接运行）。
