# CrewDock 部署说明

## 1. 环境与目录

本页命令在 Linux 服务器执行。准备 Docker Engine、Compose v2+、Git、Python 3.11+ 和可用的 HTTPS 反向代理。服务器需要能访问 GitHub、Docker Hub、crates.io、npm 和 Debian 软件源。首次构建需要编译 Rust 并安装文档转换组件，耗时取决于服务器与网络。

推荐布局：

```text
/opt/crewdock/
  source/       # 公开源代码
  .env          # 此服务器的私有配置
  postgres/     # 数据库
  redis/        # 登录限流等运行数据
  files/        # 上传文件与印章等数据
  update/       # 可选主机更新服务
  releases/     # 可选更新服务的版本源码
  backups/      # 本地备份
```

容器名统一为 `crewdock-postgres`、`crewdock-redis`、`crewdock-api`、`crewdock-web`。所有操作只针对这四个容器；共享服务器上不要全局重启或清理 Docker。

## 2. 下载与生成配置

```bash
sudo mkdir -p /opt/crewdock
sudo chown "$(id -u):$(id -g)" /opt/crewdock
git clone --branch v0.10.1 https://github.com/zhang2580384/crewdock.git /opt/crewdock/source
cd /opt/crewdock/source
python3 scripts/init-env.py --output /opt/crewdock/.env --email admin@example.com
```

将邮箱替换为自己的管理员邮箱。脚本自动生成数据库密码、32 字节主密钥、会话密钥和初始管理员密码，使用权限 `0600` 写入文件，并拒绝覆盖已有文件。它不输出密码。

编辑 `/opt/crewdock/.env`：

| 配置 | 用途 |
| --- | --- |
| `CREWDOCK_PUBLIC_BASE_URL` | 例如 `https://work.example.com`；邮件与分享地址的后备域名 |
| `CREWDOCK_WEB_BIND` | 默认 `127.0.0.1`；需要局域网访问时改为指定网卡地址或 `0.0.0.0` |
| `CREWDOCK_WEB_PORT` | 默认 `16888`；冲突时改为其他空闲端口 |
| `CREWDOCK_ROOT` | 宿主机数据目录，默认 `/opt/crewdock` |
| `CREWDOCK_NETWORK_NAME` | 默认 `crewdock` |
| `CREWDOCK_NETWORK_SUBNET` | 独立 Docker 子网，必须与现有网络和宿主机路由不重叠 |
| `CREWDOCK_BOOTSTRAP_EMAIL` / `CREWDOCK_BOOTSTRAP_PASSWORD` | 首次创建管理员时使用；后续修改此值不会重置已有账号密码 |
| `CREWDOCK_MASTER_KEY` | 加密凭据与敏感设置；丢失后无法解密，必须单独安全备份 |

启动前检查网络与端口：

```bash
docker network inspect $(docker network ls -q) --format '{{.Name}} {{range .IPAM.Config}}{{.Subnet}} {{end}}'
ip -4 route
ss -ltn
```

模板给出 `172.16.241.0/24`，这只是可修改的示例，需以当前服务器的实际检查为准。数据库和 Redis 默认不映射宿主机端口；API 在容器内监听 `16888`，只有 Web 将容器端口 `80` 映射到 `CREWDOCK_WEB_BIND:CREWDOCK_WEB_PORT`。

## 3. 构建与启动

```bash
cd /opt/crewdock/source
export CREWDOCK_GIT_SHA="$(git rev-parse --short=12 HEAD)"
export CREWDOCK_BUILD_TIME="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
docker compose --env-file /opt/crewdock/.env -f deploy/docker-compose.yml config --quiet
docker compose --env-file /opt/crewdock/.env -f deploy/docker-compose.yml up -d --build
docker compose --env-file /opt/crewdock/.env -f deploy/docker-compose.yml ps
curl -fsS http://127.0.0.1:16888/health
```

如果改了端口，健康检查也使用对应端口。成功响应的 `data.status` 为 `ok`，`data.version` 应与部署标签一致。API 启动时自动执行数据库迁移。

访问自己的域名，使用 `.env` 中的管理员邮箱及初始密码登录，再在个人账号中修改密码。首次安装没有预设站点或个人内容。SMTP、注册审批、邮件域名与模板在系统设置中配置，SMTP 凭据经主密钥加密存储。

如果构建下载较慢，可在自己的环境临时配置代理或修改 `CREWDOCK_DEBIAN_MIRROR` / `CREWDOCK_DEBIAN_SECURITY_MIRROR`。不要把代理账号或密码写入镜像、提交文件或文档。

## 4. HTTPS 反向代理

TLS 在自己的 Nginx、Caddy 或面板入口终止，将请求转发到本机 `127.0.0.1:16888`。下面是已配置证书后的 Nginx 站点片段：

```nginx
location / {
    proxy_pass http://127.0.0.1:16888;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $remote_addr;
    proxy_set_header X-Forwarded-Proto $scheme;
    client_max_body_size 200m;
    expires -1;
    proxy_cache off;
    proxy_cache_bypass 1;
    proxy_no_cache 1;
}
```

API、`/health` 和 HTML 入口不应被 CDN 或代理缓存。容器内 Nginx 已对 API 显式发送 `no-store`，只对带哈希的静态资源设置长期缓存。浏览器发起的邮件/分享操作优先使用请求 `Origin`、其次 `Referer` 的来源；无浏览器上下文时使用 `CREWDOCK_PUBLIC_BASE_URL`。

多层代理下，若要让登录限流按真实客户端 IP 生效，在 `.env` 设置至少 32 位随机字母数字的 `CREWDOCK_TRUSTED_PROXY_TOKEN`，并让最外层入口覆盖以下两个头，中间代理透传：

```nginx
proxy_set_header X-CrewDock-Client-IP $remote_addr;
proxy_set_header X-CrewDock-Proxy-Token "REPLACE_WITH_YOUR_RANDOM_TOKEN";
proxy_set_header X-Forwarded-For $remote_addr;
```

同一随机令牌只保存在私有配置中。API 校验令牌后才信任该客户端 IP。登录限制为每账号 15 分钟 10 次、每 IP 15 分钟 60 次；Redis 不可用时新登录会暂时拒绝。

图标抓取默认只访问公共地址。需要抓取自有内网服务的图标时，在 `CREWDOCK_ICON_PRIVATE_ORIGINS` 中逐一填写精确的协议、主机和端口，例如 `http://192.168.1.10:8080`；回环、链路本地、元数据和保留地址仍被拒绝。站点链接本身由用户浏览器打开。

## 5. 备份与手工升级

数据库、上传文件和 `.env`/主密钥应一起纳入备份。需要一致的文件与数据库快照时，先停止 API 和 Web 写入，备份后再启动：

```bash
cd /opt/crewdock/source
umask 077
mkdir -p /opt/crewdock/backups
stamp="$(date -u +%Y%m%d-%H%M%S)"
docker compose --env-file /opt/crewdock/.env -f deploy/docker-compose.yml stop crewdock-web crewdock-api
docker exec crewdock-postgres sh -c 'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc' > "/opt/crewdock/backups/database-$stamp.dump"
sudo tar -C /opt/crewdock -czf "/opt/crewdock/backups/private-$stamp.tar.gz" files .env
docker compose --env-file /opt/crewdock/.env -f deploy/docker-compose.yml up -d crewdock-api crewdock-web
```

在停写期间备份失败时，先检查并重新启动服务。备份包含隐私与凭据，应限制权限并复制到受保护的其他存储。

升级前阅读目标版本说明并完成备份，然后 `git fetch origin --tags`、`git switch --detach vX.Y.Z`，同步 `.env` 的 `CREWDOCK_VERSION=X.Y.Z`，重新运行第 3 节的构建和检查命令。保留旧标签及备份；涉及数据库迁移时，回退代码不等于回退数据，应在独立数据库验证恢复。

## 6. 可选：后台更新中心

Linux / systemd 主机可安装更新服务；此功能要求使用默认的 `/opt/crewdock` 布局和固定容器名。手工部署不依赖它。

```bash
cd /opt/crewdock/source
sudo sh deploy/update/install-update-service.sh
sudo systemctl start crewdock-update-check.service
sudo systemctl status crewdock-update-check.service --no-pager
```

更新器通过公开 GitHub 仓库的 HTTPS 地址读取稳定 `vX.Y.Z` 标签，无需部署密钥。每六小时检查一次；检查不会自动安装版本。管理员在系统设置发起更新后，主机服务备份数据库与源码，只重建 API/Web，校验健康与版本；失败时尝试恢复旧镜像。自动更新的镜像回退不会撤销已执行的数据库迁移。

更新器保留最近一份 `pre-*` 自动回滚备份，其他命名的手工备份不受该清理影响。API 容器只挂载受限的更新控制目录，不挂载 Docker socket、GitHub 凭据或宿主机私钥。查看 `/opt/crewdock/update/control/status.json` 和主机日志确认实际结果。

## 7. 停止与恢复

```bash
docker compose --env-file /opt/crewdock/.env -f /opt/crewdock/source/deploy/docker-compose.yml down
```

`down` 不删除宿主机数据目录。恢复时用相同版本与配置先启动数据库和 Redis，在空数据库中用 `pg_restore` 恢复 dump，恢复 `files/` 与原主密钥，再启动 API/Web。先在独立环境验证备份可恢复。
