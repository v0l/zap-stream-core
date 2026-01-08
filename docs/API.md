# zap.stream API Documentation

This document describes the REST API endpoints available in the zap.stream core streaming server.

## Authentication

The API uses NIP-98 (Nostr HTTP Auth) for authentication. All protected endpoints require an `Authorization` header with
the following format:

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

### Utility

#### Get Server Time

```
GET /api/v1/time
```

**Authentication:** Not required

**Response:**

```json
{
  "time": 1640995200000
}
```

**Description:** Returns the current server time as a Unix timestamp in milliseconds. Useful for client synchronization
and NIP-98 authentication timestamp validation.

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
      "capabilities": [
        "string"
      ],
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
    "tags": [
      "string"
    ],
    "content_warning": "string",
    "goal": "string"
  },
  "has_nwc": false
}
```

**Description:** Returns comprehensive account information including streaming endpoints with the primary stream key (
which creates a new Nostr event for each stream), account balance, terms of service status, RTMP forward destinations,
stream details, and NWC (Nostr Wallet Connect) configuration.

**Response Fields (GET):**

- `has_nwc`: Boolean indicating whether NWC is configured for this account

#### Update Account

```
PATCH /api/v1/account
```

**Authentication:** Required

**Request Body:**

```json
{
  "accept_tos": true,
  "nwc": "nostr+walletconnect://...",
  "remove_nwc": false
}
```

**Response:**

```json
{}
```

**Description:** Updates account settings, including accepting terms of service and configuring NWC (Nostr Wallet Connect) for automated withdrawals.

**NWC Configuration:**

- `nwc` (optional): A Nostr Wallet Connect URI string in the format `nostr+walletconnect://...`
  - When provided, the server will validate the NWC connection and ensure it has `pay_invoice` permissions
  - The NWC URI should be obtained from a compatible Nostr wallet that supports the NWC protocol
- `remove_nwc` (optional): Boolean flag to remove the currently configured NWC connection
  - Set to `true` to disconnect and remove the current NWC configuration
  - Cannot be used simultaneously with the `nwc` parameter
- NWC allows for automated withdrawal processing through connected Nostr wallet applications


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

**Description:** Generates a Lightning Network payment request for adding funds to the account balance. Returns a
payment request (invoice) that can be paid to credit the account.

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

**Description:** Withdraws funds from the account balance by paying a Lightning Network invoice. Returns the fee charged
and payment preimage on success.

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
  "tags": [
    "string"
  ],
  "content_warning": "string",
  "goal": "string"
}
```

**Response:**

```json
{}
```

**Description:** Updates stream event metadata such as title, description, image, tags, content warnings, and goals.

### RTMP Forward Management

#### Create RTMP Forward

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

**Description:** Creates a new RTMP forward destination. RTMP forwards allow streaming to multiple platforms
simultaneously by forwarding the stream to external RTMP endpoints (e.g., YouTube, Twitch, etc.).

#### Delete RTMP Forward

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

**Description:** Removes an RTMP forward destination by ID.

#### Update RTMP Forward

```
PATCH /api/v1/forward/{id}
```

**Authentication:** Required

**Path Parameters:**

- `id`: Forward ID to delete

**Request Body:**

```json
{
  "disabled": true
}
```

**Response:**

```json
{}
```

**Description:** Update an RTMP forward destination by ID.

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
      "created": 1704067200,
      "type": 0,
      "amount": 1000.5,
      "desc": "Lightning top-up"
    },
    {
      "created": 1704153600,
      "type": 1,
      "amount": 250.0,
      "desc": "Stream: My Live Stream"
    },
    {
      "created": 1704240000,
      "type": 1,
      "amount": 50.0,
      "desc": "Withdrawal"
    }
  ],
  "page": 0,
  "page_size": 50
}
```

**Description:** Returns paginated transaction history for the account including payments, withdrawals, and streaming
costs.

**Response Fields:**

- `created`: Unix timestamp when the transaction occurred
- `type`: Transaction type - `0` for credits (payments received, top-ups, admin credits, zaps), `1` for debits (
  withdrawals, streaming costs)
- `amount`: Transaction amount in satoshis (sats)
- `desc`: Description of the transaction - may include stream titles, "Withdrawal", "Admin Credit", or Nostr zap content

#### Get Additional Stream Keys

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

**Description:** Returns all additional stream keys for the account. These are separate from the primary stream key (
returned in account info) and are used for fixed stream events, planned streams, or 24/7 streams with pre-defined Nostr
events.

#### Create Additional Stream Key

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
    "tags": [
      "string"
    ],
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

**Description:** Creates an additional stream key with pre-defined event metadata and optional expiration time. Unlike
the primary stream key (which creates a new Nostr event each time), these keys are tied to a specific Nostr event and
are ideal for planned streams, scheduled events, or 24/7 streaming scenarios.

#### Delete Stream

```
DELETE /api/v1/stream/{id}
```

**Authentication:** Required

**Path Parameters:**

- `id`: Stream ID (UUID) to delete

**Response:**

```json
{}
```

**Description:** Deletes a stream. Users can only delete their own streams. Also publishes a Nostr deletion event if the
stream has an associated Nostr event.

### Lightning Address (LNURL)

#### LNURL Pay Endpoint

```
GET /.well-known/lnurlp/{name}
```

**Authentication:** Not required

**Path Parameters:**

- `name`: User pubkey (hex encoded)

**Response:**

```json
{
  "callback": "https://example.com/api/v1/zap/{pubkey}",
  "maxSendable": 1000000000,
  "minSendable": 1000,
  "tag": "payRequest",
  "metadata": "[[\"text/plain\", \"Zap for {pubkey}\"]]",
  "commentAllowed": null,
  "allowsNostr": true,
  "nostrPubkey": "server_pubkey_here"
}
```

**Description:** LNURL pay endpoint for Lightning Address support. Returns payment parameters for zapping a user.

#### Zap Callback

```
GET /api/v1/zap/{pubkey}
```

**Authentication:** Not required

**Path Parameters:**

- `pubkey`: Target user's pubkey (hex encoded)

**Query Parameters:**

- `amount` (required): Amount to zap in millisatoshi
- `nostr` (optional): Base64-encoded Nostr zap request event

**Response:**

```json
{
  "pr": "lnbc..."
}
```

**Description:** Handles the LNURL pay callback. Creates a Lightning invoice for zapping the specified user. Supports
Nostr zap requests for proper zap attribution.

## WebSocket API

### Real-time Metrics WebSocket

```
WS /api/v1/ws
```

**Protocol:** WebSocket

**Description:** Provides real-time streaming metrics via WebSocket connection for both streamer dashboards and admin
interfaces. Supports role-based access control with different metric types based on user permissions.

#### Authentication

WebSocket authentication uses NIP-98 (Nostr HTTP Auth) via JSON messages after connection establishment. The token
should be a base64-encoded NIP-98 event (without the "Authorization: Nostr " prefix).

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

**Authorization:** Authenticated users can subscribe to stream metrics. Regular users can only access their own streams,
while admins can access any stream.

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

**Description:** Real-time metrics for individual streams containing pipeline performance data and viewer counts.
Broadcast automatically when metrics are updated for subscribed streams. The `endpoint_stats` field contains
per-endpoint bitrate information for all active ingress and egress endpoints.

##### Node Metrics (Broadcast)

```json
{
  "type": "NodeMetrics",
  "data": {
    "node_name": "zsc-node-01",
    "cpu": 0.65,
    "memory_used": 2147483648,
    "memory_total": 8589934592,
    "uptime": 86400
  }
}
```

**Description:** Individual node performance metrics broadcast every 5 seconds for subscribed admin clients. Each
streaming node reports its own system metrics including CPU usage (as a ratio from 0.0 to 1.0), memory usage in bytes,
and uptime. Available only to admin users. Clients can aggregate data from multiple nodes to compute system-wide
statistics.

##### Error Messages

```json
{
  "type": "Error",
  "data": {
    "message": "Authentication required"
  }
}
```

#### TypeScript Types

```typescript
interface EndpointStats {
    name: string;
    bitrate: number;
}

interface ActiveStreamInfo {
    node_name: string;
    stream_id: string;
    started_at: string;
    last_segment_time: string;
    viewers: number;
    average_fps: number;
    target_fps: number;
    frame_count: number;
    endpoint_name: string;
    input_resolution: string;
    ip_address: string;
    ingress_name: string;
    endpoint_stats: Record<string, EndpointStats>;
}

interface NodeInfo {
    node_name: string;
    cpu: number;
    memory_used: number;
    memory_total: number;
    uptime: number;
}

type MetricMessage =
    | { type: "Auth"; data: { token: string } }
    | { type: "SubscribeStream"; data: { stream_id: string } }
    | { type: "SubscribeOverall"; data: null }
    | { type: "StreamMetrics"; data: ActiveStreamInfo }
    | { type: "NodeMetrics"; data: NodeInfo }
    | { type: "AuthResponse"; data: { success: boolean; is_admin: boolean; pubkey: string } }
    | { type: "Error"; data: { message: string } };
```

#### Request/Response Patterns

**Authentication Flow:**

```
Client → { type: "Auth", data: { token: "base64_nip98_event" } }
Server → { type: "AuthResponse", data: { success: true, is_admin: false, pubkey: "npub1..." } }
```

**Stream Subscription (Users):**

```
Client → { type: "SubscribeStream", data: { stream_id: "stream_123" } }
Server → { type: "StreamMetrics", data: { ... ActiveStreamInfo } } (on updates)
```

**Overall Metrics Subscription (Admins Only):**

```
Client → { type: "SubscribeOverall", data: null }
Server → { type: "NodeMetrics", data: { ... NodeInfo } } (every 5 seconds)
Server → { type: "StreamMetrics", data: { ... ActiveStreamInfo } } (on updates)
```

**Error Responses:**

```
Server → { type: "Error", data: { message: "Authentication required" } }
Server → { type: "Error", data: { message: "Access denied: You can only access your own streams" } }
Server → { type: "Error", data: { message: "Admin access required for overall metrics" } }
```

#### Connection Management

- **Automatic Reconnection:** Clients should implement automatic reconnection with exponential backoff
- **Heartbeat:** The server sends node metrics every 5 seconds; clients can detect disconnection if no messages received
  for 10+ seconds
- **Error Handling:** Always handle `Error` message types and display appropriate user feedback

#### Rate Limiting

- **Node Metrics:** Broadcast every 5 seconds for subscribed admin clients
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

The API may implement rate limiting based on account balance and usage patterns. Specific limits are not documented but
will be enforced server-side.

## CORS Support

The API includes CORS headers allowing cross-origin requests from web applications:

```
Access-Control-Allow-Origin: *
Access-Control-Allow-Headers: *
Access-Control-Allow-Methods: HEAD, GET, PATCH, DELETE, POST, OPTIONS
```

## Content Type

All API endpoints expect and return `application/json` content type unless otherwise specified.