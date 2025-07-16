# zap.stream API Documentation

This document describes the REST API endpoints available in the zap.stream core streaming server.

## Authentication

The API uses NIP-98 (Nostr HTTP Auth) for authentication. All protected endpoints require an `Authorization` header with the following format:

```
Authorization: Nostr <base64-encoded-nostr-event>
```

The Nostr event must:
- Be of kind 27235 (NIP-98 HTTP Auth)
- Have a valid signature
- Include appropriate `method` and `url` tags matching the request
- Have a recent timestamp (within acceptable time window)

## Base URL

All API endpoints are prefixed with `/api/v1/`

## Endpoints

### Account Management

#### Get Account Information
```
GET /api/v1/account
```

**Authentication:** Required

**Response:**
```json
{
  "endpoints": [
    {
      "name": "string",
      "url": "string", 
      "key": "string",
      "capabilities": ["string"],
      "cost": {
        "unit": "string",
        "rate": 0.0
      }
    }
  ],
  "balance": 0,
  "tos": {
    "accepted": false,
    "link": "string"
  },
  "forwards": [
    {
      "id": 0,
      "name": "string"
    }
  ],
  "details": {
    "title": "string",
    "summary": "string", 
    "image": "string",
    "tags": ["string"],
    "content_warning": "string",
    "goal": "string"
  }
}
```

**Description:** Returns comprehensive account information including streaming endpoints, account balance, terms of service status, forward destinations, and stream details.

#### Update Account
```
PATCH /api/v1/account
```

**Authentication:** Required

**Request Body:**
```json
{
  "accept_tos": true
}
```

**Response:** 
```json
{}
```

**Description:** Updates account settings, primarily used for accepting terms of service.

### Payment Operations

#### Request Top-up
```
GET /api/v1/topup?amount=<amount>
```

**Authentication:** Required

**Query Parameters:**
- `amount` (required): Amount to top up in millisatoshi

**Response:**
```json
{
  "pr": "string"
}
```

**Description:** Generates a Lightning Network payment request for adding funds to the account balance. Returns a payment request (invoice) that can be paid to credit the account.

#### Withdraw Funds
```
POST /api/v1/withdraw?invoice=<payment_request>
```

**Authentication:** Required

**Query Parameters:**
- `invoice` (required): Lightning Network payment request to pay

**Response:**
```json
{
  "fee": 0,
  "preimage": "string"
}
```

**Description:** Withdraws funds from the account balance by paying a Lightning Network invoice. Returns the fee charged and payment preimage on success.

### Stream Management

#### Update Stream Event
```
PATCH /api/v1/event
```

**Authentication:** Required

**Request Body:**
```json
{
  "id": "string",
  "title": "string",
  "summary": "string",
  "image": "string", 
  "tags": ["string"],
  "content_warning": "string",
  "goal": "string"
}
```

**Response:**
```json
{}
```

**Description:** Updates stream event metadata such as title, description, image, tags, content warnings, and goals.

### Forward Management

#### Create Forward
```
POST /api/v1/forward
```

**Authentication:** Required

**Request Body:**
```json
{
  "name": "string",
  "target": "string"
}
```

**Response:**
```json
{
  "id": 0
}
```

**Description:** Creates a new payment forward destination. Forwards allow automatic routing of payments to external Lightning addresses or Nostr zap targets.

#### Delete Forward
```
DELETE /api/v1/forward/{id}
```

**Authentication:** Required

**Path Parameters:**
- `id`: Forward ID to delete

**Response:**
```json
{}
```

**Description:** Removes a payment forward destination by ID.

### History and Keys

#### Get Account History
```
GET /api/v1/history
```

**Authentication:** Required

**Response:**
```json
{
  "items": [
    {
      "created": 0,
      "type": 0,
      "amount": 0.0,
      "desc": "string"
    }
  ],
  "page": 0,
  "page_size": 0
}
```

**Description:** Returns paginated transaction history for the account including payments, withdrawals, and streaming costs.

#### Get Stream Keys
```
GET /api/v1/keys
```

**Authentication:** Required

**Response:**
```json
[
  {
    "id": 0,
    "key": "string",
    "created": 0,
    "expires": 0,
    "stream_id": "string"
  }
]
```

**Description:** Returns all active stream keys for the account.

#### Create Stream Key
```
POST /api/v1/keys
```

**Authentication:** Required

**Request Body:**
```json
{
  "event": {
    "title": "string",
    "summary": "string",
    "image": "string",
    "tags": ["string"], 
    "content_warning": "string",
    "goal": "string"
  },
  "expires": "2024-01-01T00:00:00Z"
}
```

**Response:**
```json
{
  "key": "string",
  "event": "string"
}
```

**Description:** Creates a new stream key with associated event metadata and optional expiration time.

## WebSocket API

### Real-time Metrics WebSocket
```
WS /api/v1/ws
```

**Protocol:** WebSocket

**Description:** Provides real-time streaming metrics via WebSocket connection for both streamer dashboards and admin interfaces. Supports role-based access control with different metric types based on user permissions.

#### Authentication

WebSocket authentication uses NIP-98 (Nostr HTTP Auth) via JSON messages after connection establishment. The token should be a base64-encoded NIP-98 event (without the "Authorization: Nostr " prefix).

```json
{
  "type": "Auth",
  "data": {
    "token": "base64_encoded_nip98_event_here"
  }
}
```

**NIP-98 Event Requirements:**
- Event kind: 27235 (NIP-98 HTTP Auth)
- Valid signature and timestamp (within 120 seconds)
- URL tag: `ws://yourserver.com/api/v1/ws` (WebSocket URL)
- Method tag: `GET`
- The event's pubkey determines user permissions (admin status checked via database)

#### Message Types

##### Authentication Response
```json
{
  "type": "AuthResponse", 
  "data": {
    "success": true,
    "is_admin": true,
    "pubkey": "npub1..."
  }
}
```

or for regular users:
```json
{
  "type": "AuthResponse",
  "data": {
    "success": true, 
    "is_admin": false,
    "pubkey": "npub1..."
  }
}
```

##### Subscribe to Stream Metrics
```json
{
  "type": "SubscribeStream",
  "data": {
    "stream_id": "stream_123"
  }
}
```

**Authorization:** Authenticated users can subscribe to stream metrics. Regular users can only access their own streams, while admins can access any stream.

##### Subscribe to Overall Metrics
```json
{
  "type": "SubscribeOverall",
  "data": null
}
```

**Authorization:** Admin access required.

##### Stream Metrics (Broadcast)
```json
{
  "type": "StreamMetrics",
  "data": {
    "stream_id": "stream_123",
    "started_at": "2024-01-01T12:00:00Z",
    "last_segment_time": "2024-01-01T13:00:00Z",
    "viewers": 42,
    "average_fps": 30.0,
    "target_fps": 30.0,
    "frame_count": 108000,
    "endpoint_name": "Standard",
    "input_resolution": "1920x1080",
    "ip_address": "192.168.1.100",
    "ingress_name": "RTMP",
    "endpoint_stats": {
      "RTMP": {
        "name": "RTMP",
        "bitrate": 2500000
      },
      "HLS": {
        "name": "HLS",
        "bitrate": 2400000
      }
    }
  }
}
```

**Description:** Real-time metrics for individual streams containing pipeline performance data and viewer counts. Broadcast automatically when metrics are updated for subscribed streams. The `endpoint_stats` field contains per-endpoint bitrate information for all active ingress and egress endpoints.

##### Overall Metrics (Broadcast)
```json
{
  "type": "OverallMetrics",
  "data": {
    "total_streams": 5,
    "total_viewers": 127,
    "total_bandwidth": 12500000,
    "cpu_load": 1.23,
    "memory_load": 0.65,
    "uptime_seconds": 86400,
    "timestamp": 1703123456
  }
}
```

**Description:** System-wide metrics where subscribers receive real-time aggregate data computed from all active streams. The system tracks all active streams and automatically computes totals (total_streams, total_viewers, total_bandwidth) along with system performance metrics (CPU load, memory usage, uptime). Available only to admin users.

##### Error Messages
```json
{
  "type": "Error",
  "data": {
    "message": "Authentication required"
  }
}
```

#### Usage Examples

##### JavaScript Client Example
```javascript
const ws = new WebSocket('ws://localhost:8080/api/v1/ws');

ws.onopen = function() {
  // Create NIP-98 event and base64 encode it
  const nip98Event = createNIP98Event('GET', 'ws://localhost:8080/api/v1/ws');
  const token = btoa(JSON.stringify(nip98Event));
  
  // Authenticate with NIP-98 token
  ws.send(JSON.stringify({
    type: 'Auth',
    data: { token: token }
  }));
};

ws.onmessage = function(event) {
  const message = JSON.parse(event.data);
  
  switch(message.type) {
    case 'AuthResponse':
      if(message.data.success && message.data.is_admin) {
        // Subscribe to overall metrics (admin only)
        ws.send(JSON.stringify({
          type: 'SubscribeOverall',
          data: null
        }));
      }
      break;
      
    case 'OverallMetrics':
      console.log('System metrics:', message.data);
      break;
      
    case 'StreamMetrics':
      console.log('Stream metrics:', message.data);
      break;
      
    case 'Error':
      console.error('WebSocket error:', message.data.message);
      break;
  }
};

function createNIP98Event(method, url) {
  // This is a simplified example - you'd use a proper Nostr library
  // to create and sign the event with your private key
  return {
    kind: 27235,
    created_at: Math.floor(Date.now() / 1000),
    tags: [
      ['u', url],
      ['method', method]
    ],
    content: '',
    pubkey: 'your_public_key_hex',
    sig: 'your_signature_hex'
  };
}
```

##### Streamer Dashboard Example
```javascript
const ws = new WebSocket('ws://localhost:8080/api/v1/ws');

ws.onopen = function() {
  // Create NIP-98 event and authenticate
  const nip98Event = createNIP98Event('GET', 'ws://localhost:8080/api/v1/ws');
  const token = btoa(JSON.stringify(nip98Event));
  
  ws.send(JSON.stringify({
    type: 'Auth',
    data: { token: token }
  }));
};

ws.onmessage = function(event) {
  const message = JSON.parse(event.data);
  
  if(message.type === 'AuthResponse' && message.data.success) {
    // Subscribe to specific stream metrics
    ws.send(JSON.stringify({
      type: 'SubscribeStream', 
      data: { stream_id: 'your_stream_id' }
    }));
  } else if(message.type === 'StreamMetrics') {
    // Update dashboard with real-time metrics
    updateDashboard(message.data);
  }
};

function updateDashboard(metrics) {
  document.getElementById('viewer-count').textContent = metrics.viewers;
  document.getElementById('fps').textContent = metrics.average_fps;
  document.getElementById('frame-count').textContent = metrics.frame_count;
  document.getElementById('resolution').textContent = metrics.input_resolution;
  document.getElementById('ingress').textContent = metrics.ingress_name;
  
  // Display endpoint bitrates
  const endpointStats = metrics.endpoint_stats;
  for (const [name, stats] of Object.entries(endpointStats)) {
    const bitrateMbps = (stats.bitrate / 1000000).toFixed(1);
    document.getElementById(`bitrate-${name.toLowerCase()}`).textContent = `${bitrateMbps} Mbps`;
  }
}
```

#### Connection Management

- **Automatic Reconnection:** Clients should implement automatic reconnection with exponential backoff
- **Heartbeat:** The server sends overall metrics every 5 seconds; clients can detect disconnection if no messages received for 10+ seconds
- **Error Handling:** Always handle `Error` message types and display appropriate user feedback

#### Rate Limiting

- **Overall Metrics:** Broadcast every 5 seconds for subscribed admin clients
- **Stream Metrics:** Broadcast in real-time when stream metrics are updated
- **Stream Ownership:** Regular users can only access their own streams; admins can access any stream
- Each client connection can subscribe to multiple streams (admin) or specific streams they own (regular users)
- No additional rate limiting is currently implemented for WebSocket connections

## Error Handling

All endpoints return appropriate HTTP status codes:

- `200 OK` - Successful request
- `400 Bad Request` - Invalid request parameters or body
- `401 Unauthorized` - Missing or invalid authentication
- `404 Not Found` - Resource not found
- `500 Internal Server Error` - Server error

Error responses include a JSON body with error details where applicable.

## Rate Limiting

The API may implement rate limiting based on account balance and usage patterns. Specific limits are not documented but will be enforced server-side.

## CORS Support

The API includes CORS headers allowing cross-origin requests from web applications:

```
Access-Control-Allow-Origin: *
Access-Control-Allow-Headers: *
Access-Control-Allow-Methods: HEAD, GET, PATCH, DELETE, POST, OPTIONS
```

## Content Type

All API endpoints expect and return `application/json` content type unless otherwise specified.