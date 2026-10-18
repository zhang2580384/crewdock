# 共舟工作台 · CrewDock

一个可自行部署的团队工作台，把常用站点、项目文档、文件、工单、凭据和签章工具集中到同一个入口。

[部署说明](deploy/DEPLOY.md) · [参与开发](CONTRIBUTING.md) · [安全反馈](SECURITY.md) · [AGPL-3.0-or-later](LICENSE)

## 能做什么

- **站点导航**：组织、部门与个人可见范围，分类管理、书签导入、图标和收藏；支持网页、SSH、RDP 入口。
- **项目协作**：项目成员、Markdown 文档树、文件、轻量工单和有期限的内部分享。
- **组织权限**：部门、账号审批、邀请、邮件验证、角色与操作审计。
- **凭据保险库**：敏感字段加密存储，配合项目权限使用。
- **签章助手**：印章管理与授权、PDF/图片盖章、文档转换。
- **运行维护**：健康接口、版本信息、可选的主机更新服务与更新前备份。

后端使用 Rust / Axum / SQLx，前端使用 Vue 3 / TypeScript / Vite，依赖 PostgreSQL 和 Redis。当前面向单组织部署。

## Docker Compose 部署

以下命令在装有 Docker Engine、Compose v2+、Git 和 Python 3.11+ 的 **Linux 服务器**执行。

```bash
sudo mkdir -p /opt/crewdock
sudo chown "$(id -u):$(id -g)" /opt/crewdock
git clone --branch v0.10.1 https://github.com/zhang2580384/crewdock.git /opt/crewdock/source
cd /opt/crewdock/source
python3 scripts/init-env.py --output /opt/crewdock/.env --email admin@example.com
```

编辑 `/opt/crewdock/.env`，填写自己的管理员邮箱和访问域名。检查服务器现有 Docker 网络与路由，选择不重叠的 `CREWDOCK_NETWORK_SUBNET`；详见[部署说明](deploy/DEPLOY.md)。

```bash
export CREWDOCK_GIT_SHA="$(git rev-parse --short=12 HEAD)"
export CREWDOCK_BUILD_TIME="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
docker compose --env-file /opt/crewdock/.env -f deploy/docker-compose.yml up -d --build
docker compose --env-file /opt/crewdock/.env -f deploy/docker-compose.yml ps
curl -fsS http://127.0.0.1:16888/health
```

默认只监听服务器的 `127.0.0.1:16888`，通过自己的 HTTPS 反向代理访问。初始密码保存在私有 `.env` 的 `CREWDOCK_BOOTSTRAP_PASSWORD` 中，登录后可在个人账号中修改。不要把 `.env` 发到 Issue 或 Pull Request。

**全新安装只创建初始管理员、组织和默认项目，站点列表为空。** 仓库不含任何个人收藏、业务数据库、上传文件、印章、服务器账号或现成部署配置，也不携带原内部项目的 Git 历史。

## 本地开发

需要 Node.js 24+、支持当前依赖的 Rust 稳定版（Docker 构建使用 1.95）、Docker 和 Python 3.11+。

```bash
python3 scripts/init-env.py --email admin@example.com
docker compose --env-file .env -f deploy/docker-compose.yml -f deploy/docker-compose.dev.yml up -d crewdock-postgres crewdock-redis
cargo run --locked -p crewdock-api
```

另一个终端启动前端：

```bash
cd frontend
npm ci
npm run dev
```

访问 `http://127.0.0.1:5173`。API 使用 `http://127.0.0.1:16888`。本地开发的 PDF 转换和预览还需要安装 LibreOffice、Poppler 及中文字体；生产 API 镜像已包含这些工具。

## 一起修改

欢迎通过 Issue 讨论需求，通过 **Fork → 功能分支 → Pull Request 到 `main`** 贡献修改。测试、版本和提交流程见 [CONTRIBUTING.md](CONTRIBUTING.md)。

项目采用 **GNU AGPL v3 或更新版本**，允许使用、修改和商用。分发修改版，或通过网络向用户提供修改版服务时，应按协议向相应使用者提供对应源码并保留许可证。欢迎将改进提交回本仓库；AGPL 本身不强制提交 Pull Request。完整条款以 [LICENSE](LICENSE) 为准。

如果部署自己的修改版，请将界面的源码入口改为该版本的对应源码地址。
