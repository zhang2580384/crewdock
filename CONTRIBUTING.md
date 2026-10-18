# 参与共舟工作台

欢迎报告问题、改进中文文案、完善部署方式、修复缺陷和贡献功能。默认分支为 `main`，较大的功能建议先开 Issue 讨论。

## 提交修改

1. Fork 本仓库，在自己的仓库创建功能分支，例如 `feature/site-search`。
2. 按 [README](README.md) 配置本地环境，保持改动范围清晰。
3. 完成适合该改动的检查，在 Pull Request 中说明问题、改动后的行为和验证结果。
4. 向本仓库 `main` 提交 Pull Request，回应审查意见后由维护者合并。

不要在 Issue、截图、测试样例或 Pull Request 中放入真实收藏、内网域名/IP、公司数据、数据库、印章、个人账号、密码或令牌。使用 `example.com`、文档示例地址和虚构内容。

## 检查命令

从仓库根目录执行：

```bash
npm --prefix frontend ci
npm --prefix frontend test
npm --prefix frontend run build
cargo test --locked --workspace
python3 -m unittest discover -s deploy/update -p 'test_*.py'
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/verify-version.py
python3 scripts/publish-audit.py
```

Windows 可使用 `python` 替代 `python3`，并使用 `scripts/verify-version.ps1` 检查版本。GitHub Actions 会执行自动检查。新增权限、身份认证、加密、网络抓取或数据处理逻辑时，提交覆盖实际风险的回归测试。

## 发布版本

普通贡献者无需单独发布版本，由维护者统一处理：

1. 从最新 `origin/main` 整理待发布改动，保留无关的本地改动。
2. 同步 `frontend/package.json`、`backend/Cargo.toml`、`frontend/package-lock.json` 和 `Cargo.lock` 的产品版本；一般修复使用 patch，兼容功能集合使用 minor，破坏兼容的变化使用 major。
3. 更新 `.env.example` 的 `CREWDOCK_VERSION` 与 README 中推荐部署标签。
4. 完成检查，审阅 `git status --short --ignored`，确认无私有或生成文件。
5. 提交，创建附注标签 `git tag -a vX.Y.Z -m 'CrewDock vX.Y.Z'`。
6. 将已验证提交合入/快进 `main`，推送 `main` 和该标签到 `origin`，编写 Release 说明。

不要重写已公开标签或强制推送 `main`。部署后的 `/health` 验证应与代码构建、推送状态分别记录。

## 许可证

贡献代码按本仓库 **AGPL-3.0-or-later** 提供。请保留原作者和依赖的许可证信息。协议要求的源码提供义务与自愿向本项目提交 Pull Request 是两件事；欢迎将可复用的改进贡献回主线。
