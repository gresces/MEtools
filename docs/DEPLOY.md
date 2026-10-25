# 部署 me-server

目标：在个人服务器上以常驻进程运行 `me-server`，只允许认证访问，并把看板和客户端暴露在 HTTPS 后面。

## 1. 构建

在开发机或服务器上：

```bash
cargo build --release -p me-server -p me-client
```

安装二进制：

```bash
install -m 0755 target/release/me-server /usr/local/bin/me-server
install -m 0755 target/release/me-client /usr/local/bin/me-client
```

## 2. 数据库

所有数据存放在一个 SQLite 文档库里，推荐位置 `/srv/me/me.db`：

```bash
sudo install -d -o me -g me -m 0750 /srv/me
sudo -u me /usr/local/bin/me-server init --db /srv/me/me.db
sudo -u me /usr/local/bin/me-server check --db /srv/me/me.db
```

如果已有旧的 ME 仓库 `Data/`，不要在部署脚本里用 `me-server import` 完成迁移。先完成数据库初始化、密码和 systemd 启动，再按 [ME_MIGRATION.md](ME_MIGRATION.md) 的“旧读新写”流程：先用旧方法读取 `Data/`，再用 `me-client write` 逐条写入服务端，回读核对通过后才删除旧目录。

`me-server import` 只用于离线恢复和旧版本兼容，不能替代上述迁移流程。`serve`、`check`、`backup` 只打开已存在的数据库，路径错误会返回 `NOT_FOUND`，不会生成空库。库文件权限由服务端设为 `0600`，父目录 `0700`。数据库是明文 SQLite，不提供加密。

## 3. 密码

```bash
sudo install -d -m 0750 /etc/me
sudo /usr/local/bin/me-server password | sudo tee /etc/me/server.password >/dev/null
sudo chmod 0600 /etc/me/server.password
sudo chown root:root /etc/me/server.password
```

systemd 以 `me` 用户运行时，需要让 `me` 能读取密码文件：

```bash
sudo chown me:me /etc/me/server.password
sudo chmod 0400 /etc/me/server.password
```

或者使用 systemd `LoadCredential`（更严格）：

```ini
[Service]
LoadCredential=server_password:/etc/me/server.password
ExecStart=/usr/local/bin/me-server serve ... --password-file %d/server_password
```

## 4. systemd

`/etc/systemd/system/me-server.service`：

```ini
[Unit]
Description=ME personal data server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=me
Group=me
Environment=RUST_LOG=me_server=info,tower_http=warn
ExecStart=/usr/local/bin/me-server serve \
  --db /srv/me/me.db \
  --bind 127.0.0.1:8787 \
  --password-file /etc/me/server.password \
  --secure-cookie
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/srv/me
ReadOnlyPaths=/etc/me/server.password

[Install]
WantedBy=multi-user.target
```

启用：

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now me-server
sudo systemctl status me-server
curl -s http://127.0.0.1:8787/healthz
```

> `ProtectHome=true` 与 `ReadOnlyPaths` 的路径需要按实际部署调整。如果密码文件放在 `/home` 下，systemd 会读不到。

## 5. HTTPS 反向代理

### Caddy

`/etc/caddy/Caddyfile`：

```caddyfile
me.example.com {
    encode zstd gzip
    reverse_proxy 127.0.0.1:8787
}
```

```bash
sudo systemctl reload caddy
```

### Nginx

```nginx
server {
    listen 443 ssl http2;
    server_name me.example.com;

    ssl_certificate     /etc/letsencrypt/live/me.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/me.example.com/privkey.pem;

    client_max_body_size 2m;

    location / {
        proxy_pass http://127.0.0.1:8787;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

浏览器通过 HTTPS 访问时，服务端加 `--secure-cookie` 即可让 `me_session` 带 `Secure` 属性。

不要把 `8787` 直接暴露到公网。服务端默认只监听 loopback；如果必须监听非 loopback，必须显式设置密码，并且建议在防火墙层限制来源。

## 6. 客户端接入

ME agent 所在机器：

```bash
mkdir -p ~/.config/me
printf '%s\n' '<server password>' > ~/.config/me/server.password
chmod 600 ~/.config/me/server.password

cat > ~/.config/me/client.json <<'JSON'
{
  "server": "https://me.example.com",
  "token_file": "/home/me/.config/me/server.password",
  "timeout_secs": 30
}
JSON

me-client ping
me-client status
me-client summary
```

## 7. 备份与恢复

备份用 `VACUUM INTO` 生成一致快照，目标文件必须不存在：

```bash
sudo -u me /usr/local/bin/me-server backup \
  --db /srv/me/me.db \
  --to /srv/backup/me-$(date +%F).db
```

不要在服务运行时直接 `cp` 带 WAL 的 `me.db`。也可以停服务后复制 `me.db`，或使用 `restic` / `borg` 做文件级备份。

恢复：

```bash
sudo systemctl stop me-server
sudo cp /srv/backup/me-YYYY-MM-DD.db /srv/me/me.db
sudo chown me:me /srv/me/me.db
sudo chmod 0600 /srv/me/me.db
sudo -u me /usr/local/bin/me-server check --db /srv/me/me.db
sudo systemctl start me-server
```

## 8. 升级

```bash
sudo systemctl stop me-server
install -m 0755 target/release/me-server /usr/local/bin/me-server
install -m 0755 target/release/me-client /usr/local/bin/me-client
sudo -u me /usr/local/bin/me-server check --db /srv/me/me.db
sudo systemctl start me-server
curl -s http://127.0.0.1:8787/healthz
```

从 schema v4/v5/v6 升级到 v7（行动循环字段）：先备份，再在服务运行时执行：

```bash
me-client write personal.migrate --expected-version <actions version> \
  --set collection=actions --set target_schema_version=7
```

v5 为旧行动补齐 `duration_minutes=30` 和相等的 `buffer_minutes`；v7 补齐 `recurrence`、`recurrence_completed_count` 等循环字段。旧的 `calendar` 事件集合不再读写，可在备份后删除。

从 0.2.x（文件存储）升级到 0.3.x：旧进程继续按 `--data-dir` 运行。先初始化或备份新库，然后按 [ME_MIGRATION.md](ME_MIGRATION.md) 用旧方法读取旧 `Data/`、通过 `me-client write` 写入新库并回读核对；验收通过后再切换 systemd unit 的 `--db` 并重启。不要用 `me-server import` 代替迁移，验收通过前保留旧 `Data/` 目录。

## 9. 故障排查

| 现象 | 检查 |
| --- | --- |
| 启动失败：密码过短或文件缺失 | 设置 `--password-file` 或 `ME_SERVER_PASSWORD` |
| 启动失败：refusing to serve without authentication | 绑定 loopback，或显式设置密码；不要随意使用 `--allow-insecure` |
| 401 | token 值（即密码）是否一致；客户端是否读取了密码文件；反向代理是否转发 `Authorization` |
| 423 WRITE_BUSY | 是否有其他进程长时间占用写事务；等待后重试 |
| 409 VERSION_CONFLICT | 重新读取资源/记录，使用新版本重试 |
| 看板登录后仍跳回登录 | HTTPS 下是否加了 `--secure-cookie`；cookie 是否被代理丢弃 |
| 数据库损坏 | 恢复最近的 `backup` 快照；`me-server check` 会指出集合 |
| 启动失败：database does not exist | 先运行 `me-server init`；迁移旧 `Data/` 按 [ME_MIGRATION.md](ME_MIGRATION.md) 执行 |
| 看板空白 | 浏览器开发者工具查看 `/api/v1/dashboard/summary` 响应和 CSP 报错 |

日志：

```bash
journalctl -u me-server -f
```

服务端不会记录密码；如果反向代理记录请求头，请确认没有把 `Authorization` 写入访问日志。
