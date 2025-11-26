# Pipeline Log API Examples

This directory contains example clients and tools for testing the enhanced pipeline log API.

## Files

### pipeline-log-client.html

A standalone HTML/JavaScript client for testing both HTTP and WebSocket modes of the pipeline log API.

**Features:**
- WebSocket mode for real-time log tailing
- HTTP mode with customizable tail lines
- Download full log file
- Syntax highlighting (errors, warnings, info)
- Auto-scroll with user override
- Persists settings in localStorage

**Usage:**
1. Open `pipeline-log-client.html` in a web browser
2. Enter your API URL (e.g., `https://api.zap.stream`)
3. Enter the stream ID (UUID format)
4. Enter your NIP-98 authentication token
5. Select the mode (WebSocket for real-time, HTTP for one-time fetch)
6. Click "Connect"

## Testing with Command Line Tools

### cURL Examples

**Get last 200 lines (default):**
```bash
curl -H "Authorization: Nostr <token>" \
  https://api.zap.stream/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000
```

**Get last 500 lines:**
```bash
curl -H "Authorization: Nostr <token>" \
  "https://api.zap.stream/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000?tail=500"
```

**Download entire log:**
```bash
curl -H "Authorization: Nostr <token>" \
  "https://api.zap.stream/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000?download=true" \
  -o pipeline.log
```

### WebSocket Testing with websocat

Install websocat: https://github.com/vi/websocat

```bash
websocat -H "Authorization: Nostr <token>" \
  wss://api.zap.stream/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000
```

### Testing with wscat

Install wscat: `npm install -g wscat`

```bash
wscat -c "wss://api.zap.stream/api/v1/admin/pipeline-log/550e8400-e29b-41d4-a716-446655440000" \
  -H "Authorization: Nostr <token>"
```

## Python Example

```python
import asyncio
import websockets
import json

async def tail_pipeline_log(uri, auth_token, stream_id):
    headers = {
        "Authorization": f"Nostr {auth_token}"
    }
    
    url = f"{uri}/api/v1/admin/pipeline-log/{stream_id}"
    
    async with websockets.connect(url, extra_headers=headers) as websocket:
        print(f"Connected to {url}")
        
        try:
            while True:
                message = await websocket.recv()
                print(message, end='')
        except websockets.exceptions.ConnectionClosed:
            print("\nConnection closed")

# Usage
asyncio.run(tail_pipeline_log(
    "wss://api.zap.stream",
    "your-nip98-token",
    "550e8400-e29b-41d4-a716-446655440000"
))
```

## Node.js Example

```javascript
const WebSocket = require('ws');

function tailPipelineLog(uri, authToken, streamId) {
    const url = `${uri}/api/v1/admin/pipeline-log/${streamId}`;
    
    const ws = new WebSocket(url, {
        headers: {
            'Authorization': `Nostr ${authToken}`
        }
    });
    
    ws.on('open', () => {
        console.log(`Connected to ${url}`);
    });
    
    ws.on('message', (data) => {
        process.stdout.write(data.toString());
    });
    
    ws.on('error', (error) => {
        console.error('WebSocket error:', error);
    });
    
    ws.on('close', () => {
        console.log('Connection closed');
    });
}

// Usage
tailPipelineLog(
    'wss://api.zap.stream',
    'your-nip98-token',
    '550e8400-e29b-41d4-a716-446655440000'
);
```

## Troubleshooting

### Authentication Errors

Make sure your NIP-98 token is properly formatted and includes:
- Correct URL (matching the endpoint you're accessing)
- Correct HTTP method (GET for pipeline log)
- Valid timestamp (not expired)
- Proper signature

### WebSocket Connection Fails

1. Check that the server supports WebSocket upgrades
2. Verify CORS headers if connecting from a browser
3. Ensure authentication token is passed in the initial handshake
4. Check firewall/proxy settings

### Empty Log Response

If you receive an empty response or "file not found":
- The stream may not have started yet
- The stream ID might be invalid
- The log file may not have been created yet

### Large Log Files

For very large log files (>100MB):
- Use the `tail` parameter to limit the response size
- Consider using WebSocket mode which streams incrementally
- Use the `download` parameter for offline analysis
