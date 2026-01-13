# Logging Configuration

Tapedeck provides comprehensive logging capabilities with file output and runtime log level control.

## Features

- **File-based logging** with automatic daily rotation
- **Console logging** for real-time monitoring
- **Runtime log level switching** without restart
- **Dual output** - logs to both console and files simultaneously
- **Admin API** for monitoring and control

## Environment Variables

### Logging Control

```bash
# Enable/disable file logging (default: true)
ENABLE_FILE_LOGGING=true

# Enable/disable console logging (default: true)
ENABLE_CONSOLE_LOGGING=true

# Log directory path (default: ./logs)
LOG_DIR=/var/log/tapedeck

# Initial log level (default: info)
RUST_LOG=info

# Admin API port for runtime control (default: 8080)
ADMIN_PORT=8080
```

### Log Levels

Available log levels in order of verbosity:
- `ERROR` - Only errors
- `WARN` - Warnings and errors
- `INFO` - General information (default)
- `DEBUG` - Detailed debugging information
- `TRACE` - Very verbose tracing

## File Logging

### Log File Location

By default, logs are written to `./logs/` directory:
```
logs/
├── tapedeck.log              # Current day's log
├── tapedeck.log.20260113     # Previous day (rotated)
├── tapedeck.log.20260112     # Older logs
└── ...
```

### Log Rotation

- **Daily rotation**: New log file created at midnight
- **Retention**: Keeps last 7 days of logs automatically
- **Compression**: Older logs are not compressed (can be added if needed)

### File Format

File logs include detailed metadata:
```
2026-01-13T06:30:45.123456Z INFO tapedeck::sources [thread:12345] src/sources/plex.rs:234 - ✅ Plex source initialized successfully
```

Includes:
- Timestamp with microsecond precision
- Log level
- Module path
- Thread ID
- Source file and line number
- Log message

## Runtime Log Level Control

### Admin API Endpoints

The admin API runs on port 8080 (configurable via `ADMIN_PORT`).

#### Health Check

```bash
curl http://localhost:8080/health
```

Response:
```json
{
  "status": "ok",
  "service": "tapedeck-admin"
}
```

#### Get Current Log Level

```bash
curl http://localhost:8080/log-level
```

Response:
```json
{
  "current_level": "INFO",
  "message": "Current log level: INFO"
}
```

#### Set Log Level

```bash
curl -X POST http://localhost:8080/log-level \
  -H "Content-Type: application/json" \
  -d '{"level": "debug"}'
```

Response:
```json
{
  "current_level": "DEBUG",
  "message": "Log level updated to DEBUG"
}
```

**Supported levels**: `trace`, `debug`, `info`, `warn`, `error` (case-insensitive)

## Kubernetes Deployment

### Viewing Logs

**Follow deployment logs** (console output):
```bash
kubectl logs -f deployment/tapedeck -n default
```

**View logs from all pods**:
```bash
kubectl logs -l app=tapedeck --all-containers=true -n default
```

**Access log files from pod**:
```bash
# List log files
kubectl exec -it deployment/tapedeck -- ls -la /logs

# View current log file
kubectl exec -it deployment/tapedeck -- tail -f /logs/tapedeck.log

# Download log file
kubectl cp default/tapedeck-pod-xxx:/logs/tapedeck.log ./tapedeck.log
```

### Runtime Log Level Control

**Using port-forward** (recommended for security):
```bash
# Forward admin port
kubectl port-forward deployment/tapedeck 8080:8080

# In another terminal, change log level
curl -X POST http://localhost:8080/log-level \
  -H "Content-Type: application/json" \
  -d '{"level": "debug"}'
```

**Using Service** (if admin service is exposed):
```bash
curl -X POST http://tapedeck-admin-service:8080/log-level \
  -H "Content-Type: application/json" \
  -d '{"level": "debug"}'
```

### Persistent Log Storage

To persist log files across pod restarts, mount a volume:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: tapedeck
spec:
  template:
    spec:
      containers:
      - name: tapedeck
        env:
        - name: LOG_DIR
          value: "/var/log/tapedeck"
        volumeMounts:
        - name: logs
          mountPath: /var/log/tapedeck
      volumes:
      - name: logs
        persistentVolumeClaim:
          claimName: tapedeck-logs
```

## Docker Compose

```yaml
services:
  tapedeck:
    image: tapedeck:latest
    environment:
      ENABLE_FILE_LOGGING: "true"
      ENABLE_CONSOLE_LOGGING: "true"
      LOG_DIR: "/logs"
      RUST_LOG: "info"
      ADMIN_PORT: "8080"
    volumes:
      - ./logs:/logs
    ports:
      - "8080:8080"  # Admin API
```

## Troubleshooting

### No log files created

1. Check `ENABLE_FILE_LOGGING=true`
2. Verify log directory permissions
3. Check disk space
4. Look for errors in console output

### Can't change log level

1. Verify admin API is running: `curl http://localhost:8080/health`
2. Check `ADMIN_PORT` configuration
3. Ensure correct JSON format in POST request
4. Check firewall/network policies in Kubernetes

### Log level changes not reflected

1. Verify response from `/log-level` endpoint
2. Log level change is immediate, no restart needed
3. Check if you're looking at the right pod in multi-pod deployments

## Best Practices

### Development
- Use `DEBUG` level for detailed troubleshooting
- Enable both console and file logging
- Use `kubectl port-forward` for admin API access

### Production
- Start with `INFO` level
- Switch to `DEBUG` temporarily when investigating issues
- Always switch back to `INFO` after debugging
- Monitor log file sizes
- Use persistent volumes for log storage
- Restrict admin API access (use NetworkPolicies)

### Performance
- `TRACE` level generates significant overhead - use sparingly
- File logging has minimal performance impact
- Consider disabling console logging in production if using centralized logging

## Examples

### Quick debugging session

```bash
# Start with INFO level
RUST_LOG=info docker run tapedeck

# Switch to DEBUG when issue appears
curl -X POST http://localhost:8080/log-level -H "Content-Type: application/json" -d '{"level":"debug"}'

# Investigate the issue in logs
tail -f logs/tapedeck.log | grep ERROR

# Switch back to INFO
curl -X POST http://localhost:8080/log-level -H "Content-Type: application/json" -d '{"level":"info"}'
```

### Kubernetes debugging

```bash
# Check current log level
kubectl port-forward deployment/tapedeck 8080:8080 &
curl http://localhost:8080/log-level

# Enable debug logging
curl -X POST http://localhost:8080/log-level -H "Content-Type: application/json" -d '{"level":"debug"}'

# Follow logs with more details
kubectl logs -f deployment/tapedeck --tail=50

# Download full log file for analysis
POD=$(kubectl get pod -l app=tapedeck -o jsonpath='{.items[0].metadata.name}')
kubectl exec $POD -- cat /logs/tapedeck.log > debug.log

# Restore INFO level
curl -X POST http://localhost:8080/log-level -H "Content-Type: application/json" -d '{"level":"info"}'
```
