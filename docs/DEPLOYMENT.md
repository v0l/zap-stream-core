# zap.stream Production Deployment Guide

This guide covers deploying zap.stream core streaming server in a production environment.

## Architecture Overview

zap.stream requires several services to operate:

- **MariaDB/MySQL Database** - Stores user accounts, stream metadata, and transaction history
- **Strfry Nostr Relay** - Handles Nostr protocol events for stream announcements and chat
- **Blossom File Storage** (Optional) - Provides decentralized file storage for thumbnails and metadata
- **Lightning Network Daemon (LND)** - Processes Bitcoin Lightning payments
- **zap.stream Core Service** - Main streaming server handling RTMP/SRT ingestion and HLS output

## Required Services

### 1. Database (MariaDB/MySQL)

The application requires two databases:
- `zap_stream` - Main application database
- `route96` - Blossom file storage database (if using Blossom)

### 2. Nostr Relay (Strfry)

Used for publishing stream events and handling real-time communication.

### 3. Blossom File Storage (Optional)

Provides decentralized file storage for stream thumbnails and metadata.

### 4. Lightning Network Daemon

Required for processing Bitcoin payments and withdrawals.

## Docker Deployment

### Basic Docker Compose Setup

Create a `docker-compose.yml` file based on the development setup:

```yaml
version: '3.8'
services:
  # Database service
  db:
    image: mariadb:latest
    environment:
      MARIADB_ROOT_PASSWORD: ${DB_ROOT_PASSWORD}
      MARIADB_DATABASE: zap_stream
    ports:
      - "3306:3306"
    volumes:
      - db_data:/var/lib/mysql
      - ./init.sql:/docker-entrypoint-initdb.d/00-init.sql
    restart: unless-stopped

  # Nostr relay
  relay:
    image: dockurr/strfry:latest
    ports:
      - "7777:7777"
    volumes:
      - relay_data:/app/strfry-db
      - ./strfry.conf:/etc/strfry.conf
    restart: unless-stopped

  # Blossom file storage (optional)
  blossom:
    image: voidic/route96:latest
    depends_on:
      - db
    environment:
      RUST_LOG: info
    ports:
      - "8881:8000"
    volumes:
      - blossom_data:/app/data
      - ./route96.yaml:/app/config.yaml
    restart: unless-stopped

  # zap.stream core service
  zap-stream:
    image: voidic/zap-stream:latest  # Replace with actual image
    depends_on:
      - db
      - relay
    ports:
      - "8080:8080"  # HTTP server
      - "1935:1935"  # RTMP ingestion
      - "3333:3333"  # SRT ingestion
    volumes:
      - stream_output:/app/out
      - ./config.yaml:/app/config.yaml
      - ./lnd_tls.cert:/app/lnd_tls.cert
      - ./admin.macaroon:/app/admin.macaroon
    environment:
      RUST_LOG: info
    restart: unless-stopped

volumes:
  db_data:
  relay_data:
  blossom_data:
  stream_output:
```

### Environment Variables

Create a `.env` file with your production values:

```env
# Database
DB_ROOT_PASSWORD=your_secure_password_here

# Lightning Network
LND_ADDRESS=your.lnd.node:10009
LND_CERT_PATH=/app/lnd_tls.cert
LND_MACAROON_PATH=/app/admin.macaroon

# Nostr
NOSTR_PRIVATE_KEY=nsec1your_private_key_here
RELAY_URL=ws://relay:7777

# Public URLs
PUBLIC_URL=https://your-domain.com
BLOSSOM_URL=https://blossom.your-domain.com
```

### Configuration Files

#### Database Initialization (init.sql)
```sql
CREATE DATABASE IF NOT EXISTS zap_stream;
CREATE DATABASE IF NOT EXISTS route96;
```

#### zap.stream Configuration (config.yaml)
```yaml
endpoints:
  - "rtmp://0.0.0.0:1935"
  - "srt://0.0.0.0:3333"

endpoints_public_hostname: "your-domain.com"
output_dir: "/app/out"
public_url: "https://your-domain.com"
listen_http: "0.0.0.0:8080"

overseer:
  zap-stream:
    nsec: "${NOSTR_PRIVATE_KEY}"
    blossom:
      - "${BLOSSOM_URL}"
    relays:
      - "${RELAY_URL}"
    database: "mysql://root:${DB_ROOT_PASSWORD}@db:3306/zap_stream"
    lnd:
      address: "${LND_ADDRESS}"
      cert: "${LND_CERT_PATH}"
      macaroon: "${LND_MACAROON_PATH}"
```

#### Strfry Configuration (strfry.conf)
```conf
db = "/app/strfry-db/"
relay {
    bind = "0.0.0.0"
    port = 7777
    info {
        name = "zap.stream relay"
        description = "Nostr relay for zap.stream"
        pubkey = "your_relay_pubkey_here"
        contact = "admin@your-domain.com"
    }
}
```

#### Blossom Configuration (route96.yaml)
```yaml
listen: "0.0.0.0:8000"
database: "mysql://root:${DB_ROOT_PASSWORD}@db:3306/route96"
storage_dir: "/app/data"
max_upload_bytes: 5000000000  # 5GB
public_url: "${BLOSSOM_URL}"
```

## Production Deployment Steps

### 1. Prerequisites

- Docker and Docker Compose installed
- Lightning Network node (LND) running and accessible
- Domain name and SSL certificates configured
- Reverse proxy (nginx/traefik) for SSL termination

### 2. Initial Setup

```bash
# Clone the repository
git clone https://github.com/v0l/zap-stream-core.git
cd zap-stream-core

# Create configuration directory
mkdir -p production
cd production

# Copy configuration templates
cp ../crates/zap-stream/docker-compose.yml ./
cp ../crates/zap-stream/dev-setup/* ./

# Edit configuration files for production
nano config.yaml
nano .env
nano strfry.conf
nano route96.yaml
```

### 3. SSL and Reverse Proxy

Configure your reverse proxy to handle SSL termination:

#### Nginx Example
```nginx
server {
    listen 443 ssl http2;
    server_name your-domain.com;
    
    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;
    
    # Main application
    location / {
        proxy_pass http://localhost:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
    
    # WebSocket for Nostr relay
    location /relay {
        proxy_pass http://localhost:7777;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}

# Blossom file storage
server {
    listen 443 ssl http2;
    server_name blossom.your-domain.com;
    
    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;
    
    location / {
        proxy_pass http://localhost:8881;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        client_max_body_size 5G;
    }
}
```

### 4. Start Services

```bash
# Start all services
docker-compose up -d

# Check logs
docker-compose logs -f

# Verify services are running
docker-compose ps
```

### 5. Health Checks

Verify each service is working:

```bash
# Database
docker-compose exec db mysql -u root -p -e "SHOW DATABASES;"

# Relay
curl -H "Accept: application/nostr+json" http://localhost:7777

# Blossom (if configured)
curl http://localhost:8881/health

# zap.stream API
curl http://localhost:8080/api/v1/health
```

## Security Considerations

### 1. Network Security
- Use firewall rules to restrict access to internal services
- Only expose necessary ports publicly
- Use SSL/TLS for all external communication

### 2. Secrets Management
- Store sensitive configuration in environment variables
- Use Docker secrets for production deployments
- Rotate keys and passwords regularly

### 3. Lightning Network Security
- Secure LND node properly
- Use read-only macaroons where possible
- Monitor channel balances and routing

### 4. Database Security
- Use strong passwords
- Regular backups
- Enable SSL connections
- Restrict network access

## Monitoring and Logging

### 1. Application Logs
```bash
# View all service logs
docker-compose logs

# Follow specific service logs  
docker-compose logs -f zap-stream
```

### 2. Health Monitoring
Set up monitoring for:
- Service availability
- Database connections
- Lightning Network connectivity
- Disk space for stream output
- Memory and CPU usage

### 3. Metrics Collection
Consider integrating with:
- Prometheus for metrics collection
- Grafana for visualization
- AlertManager for notifications

## Backup and Recovery

### 1. Database Backup
```bash
# Backup database
docker-compose exec db mysqldump -u root -p zap_stream > backup.sql

# Restore database
docker-compose exec -T db mysql -u root -p zap_stream < backup.sql
```

### 2. Stream Data Backup
```bash
# Backup stream output directory
tar -czf stream_backup_$(date +%Y%m%d).tar.gz stream_output/
```

### 3. Configuration Backup
```bash
# Backup all configuration
tar -czf config_backup_$(date +%Y%m%d).tar.gz *.yaml *.conf .env
```

## Troubleshooting

### Common Issues

1. **Database Connection Failed**
   - Check database service is running
   - Verify connection string in config
   - Check firewall/network connectivity

2. **Lightning Payments Not Working**
   - Verify LND connectivity
   - Check macaroon permissions
   - Ensure sufficient channel capacity

3. **Streams Not Playing**
   - Check RTMP/SRT ingestion ports
   - Verify output directory permissions
   - Check HLS segment generation

4. **High CPU/Memory Usage**
   - Monitor concurrent streams
   - Adjust encoding settings
   - Scale horizontally if needed

### Log Analysis
```bash
# Check for errors in logs
docker-compose logs | grep -i error

# Monitor resource usage
docker stats

# Check disk space
df -h
```

## Scaling

For high-traffic deployments:

1. **Database Scaling**
   - Use read replicas
   - Implement database sharding
   - Connection pooling

2. **Stream Processing**
   - Multiple zap-stream instances
   - Load balancing
   - CDN for HLS delivery

3. **Storage Scaling**
   - Distributed file storage
   - Regular cleanup of old segments
   - Separate storage for live vs archived content

## Support

For issues and questions:
- GitHub Issues: https://github.com/v0l/zap-stream-core/issues
- Documentation: https://github.com/v0l/zap-stream-core/docs/