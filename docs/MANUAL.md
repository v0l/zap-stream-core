# Manual Setup (Ubuntu)

How to deploy `zap-stream` on a clean linux server.

## Dependencies
Install [rustup](https://rustup.rs) if you don't already have it.

Install build dependencies:
```bash
apt install \
    build-essential \
    make \
    autoconf \
    pkg-config \
    libssl-dev \
    libclang-dev \
    libavutil-dev \
    libavformat-dev \
    libavfilter-dev \
    libavdevice-dev \
    libavcodec-dev \
    libswscale-dev \
    libx264-dev \
    libwebp-dev \
    protobuf-compiler
```

Create a dedicated user:
```bash
useradd zap-stream
mkdir -p /usr/share/zap-stream
chown zap-stream:zap-stream /usr/share/zap-stream
```

## Install `zap-stream`
```bash
cargo install zap-stream --git https://github.com/v0l/zap-stream-core --root /usr/local
```

Minimal config, copy it to `/usr/share/zap-stream/config.yaml`:
```yaml
endpoints:
  - "rtmp://0.0.0.0:1935"
endpoints_public_hostname: "my.stream.server"
output_dir: "/usr/share/zap-stream/out"
public_url: "http://my.stream.server"
listen_http: "0.0.0.0:80"
overseer:
  nsec: "nsec1234"
  relays:
    - "wss://relay.damus.io"
    - "wss://relay.snort.social"
    - "wss://relay.primal.net"
    - "wss://nos.lol"
  database: "mysql://zap-stream:zap-stream@localhost:3306/zap_stream?max_connections=2"
  # LND config in (zap-stream build only)
  lnd:
    address: "https://127.0.0.1:10001"
    cert: "tls.cert"
    macaroon: "admin.macaroon"
```

## Install `mariadb`
```bash
apt install mariadb-server
```

Setup database
```mysql
create user 'zap-stream'@'localhost' identified by 'zap-stream';
create database zap_stream;
grant all privileges on zap_stream.* to 'zap-stream'@'localhost';
flush privileges;                                        
```

## SystemD service
Setup systemd service `/etc/systemd/system/zap-stream.service`:
```
[Unit]
Description=zap-stream

[Service]
Type=simple
User=zap-stream
Group=zap-stream
WorkingDirectory=/usr/share/zap-stream
Environment="RUST_LOG=info"
ExecStart=/usr/local/bin/zap-stream

[Install]
WantedBy=network.target
```

## Admin UI
The admin UI lets you manage the users and endpoint configurations

Install Node.js if you dont already have it:

```bash
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
source ~/.nvm/nvm.sh
nvm install 22
```

Clone the admin UI and build it
```bash
git clone https://github.com/v0l/zap-stream-admin.git
cd zap-stream-admin
npx yarn
VITE_API_BASE_URL=http://my.stream.server npx yarn build
```

Copy the site to the zap-stream dir
```bash
mv build /usr/share/zap-stream/admin
```

# Nginx Proxy
To serve both the API and Admin UI, as well as SSL certs, you can use nginx as a reverse proxy to the service.

Install nginx:
```bash
apt install nginx
```

Configure proxy:
```conf
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
server {
    listen 80;
    listen [::]:80;
    
    server_name my.stream.server;
    proxy_read_timeout 600;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }

    location /api/v1/ws {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}

server {
    listen 80;
    listen [::]:80;
    
    server_name admin.my.stream.server;
    root /usr/share/zap-stream/admin;
    
    location / {
        try_files $uri $uri/ index.html =404;
    }
}
```

Configure zap-stream to listen on localhost instead of the public interface:
```yaml
listen_http: "127.0.0.1:8080"
```

Open your admin site: `http://admin.my.stream.server` and login to insert your pubkey in the database

## Admin setup
Once you login to the admin dashboard you might see that you dont have permissions, you can mark yourself as admin using SQL command:

Start sql cli:
```bash
mariadb -D zap-stream
```

Update user `1` (assuming you are the first person to open admin page) to admin:
```sql
update user set is_admin = 1 where id = 1;
```

You can check the user id by just selecting all users from the table first:
```sql
select id,hex(pubkey),is_admin from user;
```