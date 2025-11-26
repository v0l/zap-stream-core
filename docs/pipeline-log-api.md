# Pipeline Log API

This document describes the enhanced pipeline log API endpoint that supports multiple modes of operation.

## Endpoint

`GET /api/v1/admin/pipeline-log/{stream_id}`

## Authentication

All requests require admin authentication using NIP-98. Include the authentication token in the `Authorization` header.

## HTTP Mode (Default)

### Basic Usage

Returns the last 200 lines of the pipeline log by default:

```bash
curl -H "Authorization: Nostr <base64-encoded-nip98-event>" \
  https://api.example.com/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000
```

### Query Parameters

#### `tail` (optional)

Specify the number of lines to return from the end of the log file.

Example - Get last 500 lines:
```bash
curl -H "Authorization: Nostr <base64-encoded-nip98-event>" \
  "https://api.example.com/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000?tail=500"
```

#### `download` (optional)

Set to `true` to download the entire log file. The response will include a `Content-Disposition` header with a suggested filename.

Example - Download entire log:
```bash
curl -H "Authorization: Nostr <base64-encoded-nip98-event>" \
  "https://api.example.com/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000?download=true" \
  -o pipeline.log
```

### Response

**Content-Type:** `text/plain; charset=utf-8`

**Success (200 OK):**
```
[2024-01-01 12:00:00] Pipeline starting...
[2024-01-01 12:00:01] Video stream detected: 1920x1080 @ 30fps
[2024-01-01 12:00:02] Audio stream detected: 48000Hz stereo
...
```

**Not Found (404):**
```
Pipeline log file not found. This may be because the stream has not been started yet or the stream ID is invalid.
```

## WebSocket Mode

The same endpoint supports WebSocket connections for real-time log tailing.

### Connection

Upgrade the HTTP connection to WebSocket using the same endpoint URL:

```javascript
const ws = new WebSocket(
  'wss://api.example.com/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000',
  // Include NIP-98 auth token in request headers
  { headers: { 'Authorization': 'Nostr <base64-encoded-nip98-event>' } }
);

ws.onopen = () => {
  console.log('Connected to pipeline log stream');
};

ws.onmessage = (event) => {
  console.log('Log:', event.data);
  // Each message contains one or more log lines
};

ws.onerror = (error) => {
  console.error('WebSocket error:', error);
};

ws.onclose = () => {
  console.log('Disconnected from pipeline log stream');
};
```

### Behavior

1. **Initial Content**: Upon connection, the WebSocket sends the last 200 lines of existing log content
2. **Real-time Updates**: New log lines are streamed as they are written to the file
3. **Polling Interval**: The server checks for new content every 100ms
4. **Automatic Cleanup**: The connection closes automatically if the client disconnects or if an error occurs

### Example with curl

```bash
# Note: This requires curl with WebSocket support (curl 7.86+)
curl --include \
  --no-buffer \
  --header "Connection: Upgrade" \
  --header "Upgrade: websocket" \
  --header "Sec-WebSocket-Version: 13" \
  --header "Sec-WebSocket-Key: SGVsbG8sIHdvcmxkIQ==" \
  --header "Authorization: Nostr <base64-encoded-nip98-event>" \
  "https://api.example.com/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000"
```

### Example with websocat

```bash
websocat -H "Authorization: Nostr <base64-encoded-nip98-event>" \
  wss://api.example.com/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000
```

## Security

- **Authentication Required**: All requests (HTTP and WebSocket) require valid admin authentication
- **Path Traversal Protection**: Stream ID is validated as a UUID to prevent path traversal attacks
- **Audit Logging**: All access to pipeline logs is recorded in the admin audit log

## Error Handling

### Invalid Stream ID

**HTTP 400 Bad Request**
```
Invalid stream_id format, must be a valid UUID
```

### File Not Found

**HTTP 200 OK** (with message in body)
```
Pipeline log file not found. This may be because the stream has not been started yet or the stream ID is invalid.
```

### Authentication Failure

**HTTP 401 Unauthorized**
```json
{
  "error": "Authentication failed"
}
```

### Permission Denied

**HTTP 403 Forbidden**
```json
{
  "error": "Access denied: Admin privileges required"
}
```

## Use Cases

### Development and Debugging

Use the HTTP endpoint to quickly check recent log entries:
```bash
# Get last 50 lines
curl "...?tail=50"
```

### Long-term Analysis

Download the entire log for offline analysis:
```bash
# Download complete log
curl "...?download=true" -o stream-logs.log
```

### Real-time Monitoring

Use WebSocket for live monitoring of active streams:
```javascript
// Monitor multiple streams simultaneously
const streams = ['stream-id-1', 'stream-id-2', 'stream-id-3'];
streams.forEach(streamId => {
  const ws = new WebSocket(`wss://api.example.com/api/v1/admin/pipeline-log/${streamId}`);
  ws.onmessage = (event) => {
    console.log(`[${streamId}]`, event.data);
  };
});
```

## Implementation Notes

- Log files are stored in `{output_dir}/{stream_id}/pipeline.log`
- The WebSocket implementation uses tokio's async file reading with buffering
- Log lines are sent to WebSocket clients as they are written (100ms polling interval)
- The endpoint is designed to handle large log files efficiently by reading only the necessary tail portion
