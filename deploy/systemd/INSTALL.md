# Systemd Installation Guide

This guide explains how to install and run Tapedeck as a systemd service on Linux.

## Prerequisites

- Linux system with systemd (most modern distributions)
- Rust toolchain (for building) or pre-built binary
- Plex Media Server accessible from your system
- ListenBrainz or Last.fm account with API token

## Installation Steps

### 1. Create System User

Create a dedicated user for running Tapedeck:

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin tapedeck
```

### 2. Create Directory Structure

Set up the required directories:

```bash
# Application directory
sudo mkdir -p /var/tapedeck/data

# Log directory
sudo mkdir -p /var/log/tapedeck

# Configuration directory
sudo mkdir -p /etc/tapedeck

# Set ownership
sudo chown -R tapedeck:tapedeck /var/tapedeck
sudo chown -R tapedeck:tapedeck /var/log/tapedeck
sudo chown -R tapedeck:tapedeck /etc/tapedeck
```

### 3. Build or Download Binary

**Option A: Build from source**

```bash
# Clone repository
git clone https://github.com/abalajiksh/tapedeck.git
cd tapedeck

# Build release binary
cargo build --release

# Copy binary to installation directory
sudo cp target/release/tapedeck /var/tapedeck/
sudo chown tapedeck:tapedeck /var/tapedeck/tapedeck
sudo chmod +x /var/tapedeck/tapedeck
```

**Option B: Download pre-built binary** (if available)

```bash
# Download and extract binary
wget https://github.com/abalajiksh/tapedeck/releases/latest/download/tapedeck-linux-x86_64.tar.gz
tar xzf tapedeck-linux-x86_64.tar.gz

# Copy to installation directory
sudo cp tapedeck /var/tapedeck/
sudo chown tapedeck:tapedeck /var/tapedeck/tapedeck
sudo chmod +x /var/tapedeck/tapedeck
```

### 4. Configure Environment

Create the environment configuration file:

```bash
# Copy example configuration
sudo cp deploy/systemd/tapedeck.env.example /etc/tapedeck/tapedeck.env

# Edit configuration with your settings
sudo nano /etc/tapedeck/tapedeck.env
```

**Required settings to update:**

```bash
# Plex configuration
PLEX_URL=http://your-plex-server:32400
PLEX_TOKEN=your-actual-plex-token

# ListenBrainz configuration
LISTENBRAINZ_TOKEN=your-actual-listenbrainz-token

# Optional: Custom ListenBrainz URL (for self-hosted instances)
# LISTENBRAINZ_BASE_URL=https://your-listenbrainz.example.com
```

**How to get your Plex token:**
1. Open Plex Web App
2. Play any item
3. Click the three dots (...) > "Get Info"
4. Click "View XML"
5. Look for `X-Plex-Token` in the URL

Alternatively: https://support.plex.tv/articles/204059436-finding-an-authentication-token-x-plex-token/

**How to get your ListenBrainz token:**
1. Go to https://listenbrainz.org/profile/
2. Scroll to "User Tokens" section
3. Copy your user token

### 5. Install Systemd Service

Install the service file:

```bash
# Copy service file
sudo cp deploy/systemd/tapedeck.service /etc/systemd/system/

# Reload systemd to recognize new service
sudo systemctl daemon-reload
```

### 6. Start and Enable Service

Start the service and enable it to run at boot:

```bash
# Start service
sudo systemctl start tapedeck

# Enable service to start at boot
sudo systemctl enable tapedeck

# Check status
sudo systemctl status tapedeck
```

## Managing the Service

### Service Commands

```bash
# Start service
sudo systemctl start tapedeck

# Stop service
sudo systemctl stop tapedeck

# Restart service
sudo systemctl restart tapedeck

# Check status
sudo systemctl status tapedeck

# View logs (real-time)
sudo journalctl -u tapedeck -f

# View logs (last 100 lines)
sudo journalctl -u tapedeck -n 100

# View logs since boot
sudo journalctl -u tapedeck -b
```

### Viewing Logs

**Console logs (journalctl):**
```bash
sudo journalctl -u tapedeck -f
```

**File logs:**
```bash
# Current log file
sudo tail -f /var/log/tapedeck/tapedeck.log

# List all log files
sudo ls -lh /var/log/tapedeck/

# View specific date
sudo cat /var/log/tapedeck/tapedeck.log.20260113
```

### Runtime Log Level Control

**Check current log level:**
```bash
curl http://localhost:8080/log-level
```

**Switch to debug logging:**
```bash
curl -X POST http://localhost:8080/log-level \
  -H "Content-Type: application/json" \
  -d '{"level": "debug"}'
```

**Switch back to info:**
```bash
curl -X POST http://localhost:8080/log-level \
  -H "Content-Type: application/json" \
  -d '{"level": "info"}'
```

## Configuration Updates

### Updating Environment Variables

```bash
# Edit configuration
sudo nano /etc/tapedeck/tapedeck.env

# Restart service to apply changes
sudo systemctl restart tapedeck
```

### Updating Binary

```bash
# Stop service
sudo systemctl stop tapedeck

# Backup old binary (optional)
sudo cp /var/tapedeck/tapedeck /var/tapedeck/tapedeck.backup

# Copy new binary
sudo cp /path/to/new/tapedeck /var/tapedeck/
sudo chown tapedeck:tapedeck /var/tapedeck/tapedeck
sudo chmod +x /var/tapedeck/tapedeck

# Start service
sudo systemctl start tapedeck
```

## Troubleshooting

### Service won't start

**Check service status:**
```bash
sudo systemctl status tapedeck
```

**Check logs for errors:**
```bash
sudo journalctl -u tapedeck -n 50
```

**Common issues:**

1. **Permission denied**: Ensure binary is executable
   ```bash
   sudo chmod +x /var/tapedeck/tapedeck
   ```

2. **Directory permissions**: Check ownership
   ```bash
   sudo chown -R tapedeck:tapedeck /var/tapedeck /var/log/tapedeck
   ```

3. **Invalid configuration**: Verify environment file
   ```bash
   sudo cat /etc/tapedeck/tapedeck.env
   ```

4. **Port already in use**: Check if port 8080 is available
   ```bash
   sudo ss -tulpn | grep 8080
   ```

### Database issues

**Reset databases:**
```bash
sudo systemctl stop tapedeck
sudo rm /var/tapedeck/data/*.db
sudo systemctl start tapedeck
```

### Can't connect to Plex

1. Verify Plex URL is accessible:
   ```bash
   curl http://your-plex-server:32400/identity
   ```

2. Test with Plex token:
   ```bash
   curl "http://your-plex-server:32400/library/sections?X-Plex-Token=YOUR_TOKEN"
   ```

3. Check firewall rules if Plex is remote

### ListenBrainz connection issues

1. Verify token is correct
2. Check network connectivity:
   ```bash
   curl https://api.listenbrainz.org/1/stats/sitewide/artists
   ```

## Security Considerations

### File Permissions

The service file includes security hardening:
- Runs as unprivileged user `tapedeck`
- Read-only filesystem except for data and log directories
- Restricted system calls
- Memory limits
- No new privileges allowed

### Firewall Configuration

If you need remote access to the admin API:

```bash
# Allow admin API (port 8080) - use with caution!
sudo ufw allow 8080/tcp

# Better: Only allow from specific IP
sudo ufw allow from 192.168.1.0/24 to any port 8080
```

**Recommendation**: Keep admin API on localhost and use SSH tunnel for remote access:

```bash
# From remote machine
ssh -L 8080:localhost:8080 user@tapedeck-server

# Then access locally
curl http://localhost:8080/log-level
```

### Protecting Tokens

Ensure configuration file is not world-readable:

```bash
sudo chmod 600 /etc/tapedeck/tapedeck.env
sudo chown tapedeck:tapedeck /etc/tapedeck/tapedeck.env
```

## Uninstallation

To completely remove Tapedeck:

```bash
# Stop and disable service
sudo systemctl stop tapedeck
sudo systemctl disable tapedeck

# Remove service file
sudo rm /etc/systemd/system/tapedeck.service
sudo systemctl daemon-reload

# Remove files
sudo rm -rf /var/tapedeck
sudo rm -rf /var/log/tapedeck
sudo rm -rf /etc/tapedeck

# Remove user
sudo userdel tapedeck
```

## Monitoring

### Health Check Script

Create a simple health check:

```bash
#!/bin/bash
# /usr/local/bin/tapedeck-health-check.sh

if curl -s http://localhost:8080/health | grep -q '"status":"ok"'; then
    echo "Tapedeck is healthy"
    exit 0
else
    echo "Tapedeck health check failed"
    exit 1
fi
```

### Systemd Timer for Health Checks

You can create a systemd timer to periodically check health and restart if needed.

## Additional Resources

- [Main Documentation](https://github.com/abalajiksh/tapedeck)
- [Logging Documentation](../../docs/LOGGING.md)
- [Plex API Documentation](https://www.plexopedia.com/plex-media-server/api/)
- [ListenBrainz API Documentation](https://listenbrainz.readthedocs.io/)
