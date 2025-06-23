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