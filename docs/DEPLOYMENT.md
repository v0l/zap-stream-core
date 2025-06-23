# zap.stream Production Deployment Guide

This guide covers deploying zap.stream core streaming server in a production environment.

## Architecture Overview

zap.stream requires several services to operate:

- **MariaDB/MySQL Database** - Stores user accounts, stream metadata, and transaction history
- **Public Nostr Relays** - Uses existing public Nostr relays for stream announcements and chat
- **Public Blossom Servers** (Optional) - Uses existing public Blossom servers for decentralized file storage
- **Lightning Network Daemon (LND)** - Processes Bitcoin Lightning payments
- **zap.stream Core Service** - Main streaming server handling RTMP/SRT ingestion and HLS output

## Required Services

### 1. Database (MariaDB/MySQL)

The application requires a database for storing user accounts, stream metadata, and transaction history:
- `zap_stream` - Main application database

### 2. Public Nostr Relays

zap.stream connects to existing public Nostr relays for publishing stream events and handling real-time communication. Popular public relays include:
- `wss://relay.damus.io`
- `wss://nos.lol` 
- `wss://relay.snort.social`

### 3. Public Blossom Servers (Optional)

For file storage, zap.stream can use existing public Blossom servers for thumbnails and metadata. Popular public Blossom servers include:
- `https://blossom.oxtr.dev`
- `https://cdn.satellite.earth`

### 4. Lightning Network Daemon

Required for processing Bitcoin payments and withdrawals. You'll need access to an LND node.

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

  # zap.stream core service
  zap-stream:
    image: voidic/zap-stream:latest  # Replace with actual image
    depends_on:
      - db
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

# Nostr (public relays)
NOSTR_PRIVATE_KEY=nsec1your_private_key_here
RELAY_URL=wss://relay.damus.io

# Blossom (public servers)
BLOSSOM_URL=https://blossom.oxtr.dev

# Public URLs
PUBLIC_URL=https://your-domain.com
```

### Configuration Files

#### Database Initialization (init.sql)
```sql
CREATE DATABASE IF NOT EXISTS zap_stream;
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
      - "wss://nos.lol"
      - "wss://relay.snort.social"
    database: "mysql://root:${DB_ROOT_PASSWORD}@db:3306/zap_stream"
    lnd:
      address: "${LND_ADDRESS}"
      cert: "${LND_CERT_PATH}"
      macaroon: "${LND_MACAROON_PATH}"
```


## Production Deployment Steps

### 1. Prerequisites

- Docker and Docker Compose installed
- Lightning Network node (LND) running and accessible
- Domain name and SSL certificates configured
- Reverse proxy (nginx/traefik) for SSL termination
- Access to public Nostr relays (most are free to use)
- Access to public Blossom servers (optional, for file storage)

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
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
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

# zap.stream API
curl http://localhost:8080/api/v1/health

# Test Nostr relay connectivity (using public relay)
curl -H "Accept: application/nostr+json" https://relay.damus.io

# Test Blossom server connectivity (using public server)
curl https://blossom.oxtr.dev
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
- Public Nostr relay connectivity
- Public Blossom server connectivity (if used)
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