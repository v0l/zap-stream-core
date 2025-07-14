# Zap Stream Admin API Documentation

This document describes the Admin API endpoints for the Zap Stream platform. These endpoints allow administrators to manage users, balances, and platform settings.

## Authentication

All admin endpoints require **NIP-98 Nostr HTTP Authentication** with admin privileges:

1. **NIP-98 Authentication**: Requests must include proper Nostr event signatures in headers
2. **Admin Access**: The authenticated user must have `is_admin = true` in the database
3. **Access Denied**: Non-admin users receive `Access denied: Admin privileges required` error

## Base URL

All endpoints are prefixed with `/api/v1/admin/`

## Endpoints

### 1. List Users

**Endpoint**: `GET /api/v1/admin/users`

**Description**: Retrieve a paginated list of users with optional search functionality.

**Query Parameters**:
- `page` (optional, default: 0): Page number for pagination
- `limit` (optional, default: 50): Number of users per page
- `search` (optional): Search term to filter users by public key (hex prefix match)

**Example Requests**:
```http
GET /api/v1/admin/users
GET /api/v1/admin/users?page=1&limit=25
GET /api/v1/admin/users?search=02a1b2c3
```

**Response Format**:
```json
{
  "users": [
    {
      "id": 123,
      "pubkey": "02a1b2c3d4e5f6...",
      "created": 1640995200,
      "balance": 50000,
      "is_admin": false,
      "is_blocked": false,
      "tos_accepted": 1640995300,
      "title": "User's Stream Title",
      "summary": "Stream description"
    }
  ],
  "page": 0,
  "limit": 50,
  "total": 1
}
```

**Response Fields**:
- `users`: Array of user objects
- `page`: Current page number
- `limit`: Number of users per page
- `total`: Total number of users returned
- `id`: Internal user ID
- `pubkey`: User's Nostr public key (hex encoded)
- `created`: Unix timestamp of account creation
- `balance`: User's balance in millisatoshis
- `is_admin`: Whether user has admin privileges
- `is_blocked`: Whether user is blocked
- `tos_accepted`: Unix timestamp when ToS was accepted (null if not accepted)
- `title`: User's default stream title
- `summary`: User's default stream summary

### 2. Manage User

**Endpoint**: `POST /api/v1/admin/users/{id}`

**Description**: Perform administrative actions on a specific user account.

**Path Parameters**:
- `id`: User ID (numeric)

**Request Body**:
```json
{
  "set_admin": true,
  "set_blocked": false,
  "add_credit": 25000,
  "memo": "Admin credit for testing",
  "title": "New Stream Title",
  "summary": "Updated stream description", 
  "image": "https://example.com/image.jpg",
  "tags": ["gaming", "music"],
  "content_warning": "Adult content",
  "goal": "Raise funds for charity"
}
```

**Request Fields** (all optional):
- `set_admin`: Boolean to grant/revoke admin privileges
- `set_blocked`: Boolean to block/unblock user
- `add_credit`: Amount in millisatoshis to add to user's balance
- `memo`: Description for the credit transaction (currently not stored)
- `title`: Update user's default stream title
- `summary`: Update user's default stream summary
- `image`: Update user's default stream image URL
- `tags`: Array of tags for user's default stream
- `content_warning`: Content warning for user's streams
- `goal`: User's streaming goal description

**Response**: 
```json
{}
```
Empty JSON object on success.

**Example Operations**:

**Grant Admin Privileges**:
```json
{
  "set_admin": true
}
```

**Add Credits**:
```json
{
  "add_credit": 100000,
  "memo": "Promotional credit"
}
```

**Block User**:
```json
{
  "set_blocked": true
}
```

**Update Stream Defaults**:
```json
{
  "title": "Official Stream",
  "tags": ["official", "verified"],
  "goal": "Educational content"
}
```

## Error Responses

**Authentication Errors**:
```json
{
  "error": "Access denied: Admin privileges required"
}
```

**Validation Errors**:
```json
{
  "error": "Missing user ID"
}
```

**Not Found**:
```json
{
  "error": "User not found"
}
```

## Implementation Notes

### Credit System
- All amounts are in **millisatoshis** (1 sat = 1000 msat)
- Credits are processed immediately and create payment records
- Payment records track admin credits for audit purposes

### Stream Defaults
- Stream defaults are applied to new streams created by the user
- Existing active streams are not affected by default changes
- Tags are stored as comma-separated strings in the database

### Search Functionality
- Search matches against the hexadecimal representation of public keys
- Partial prefix matching is supported (e.g., searching "02a1" matches "02a1b2c3...")
- Search is case-insensitive
- Limited to 50 results maximum

### Database Operations
- All operations are atomic and use database transactions where appropriate
- Admin credit operations create proper payment audit trails
- User balance updates are handled safely with proper validation

## Security Considerations

1. **Admin Access Control**: Always verify admin status before executing operations
2. **Input Validation**: Validate all numeric inputs (user IDs, credit amounts)
3. **Audit Trail**: Admin operations should be logged for compliance
4. **Rate Limiting**: Consider implementing rate limits for admin endpoints
5. **Authentication**: Ensure NIP-98 signature validation is properly implemented

## UI Implementation Guidelines

When building a web UI for these endpoints:

1. **User List Table**: Display users with sortable columns (ID, balance, created date)
2. **Search Box**: Implement live search with debouncing for public key lookup
3. **Pagination Controls**: Standard page navigation with configurable page sizes
4. **User Actions**: Modal dialogs or forms for user management operations
5. **Balance Display**: Show balances in sats (divide msat by 1000) for user readability
6. **Confirmation Dialogs**: Require confirmation for destructive actions (blocking, admin changes)
7. **Success/Error Messages**: Clear feedback for all operations
8. **Admin Indicators**: Visual indicators for admin users and blocked users
9. **Stream Key Management**: Include buttons to view and regenerate stream keys with proper confirmation
10. **Stream Key Display**: Show stream keys in a copyable format with masking for security

### 3. List User Streams

**Endpoint**: `GET /api/v1/admin/users/{id}/streams`

**Description**: Retrieve a paginated list of streams for a specific user.

**Path Parameters**:
- `id`: User ID (numeric)

**Query Parameters**:
- `page` (optional, default: 0): Page number for pagination
- `limit` (optional, default: 50): Number of streams per page

**Example Requests**:
```http
GET /api/v1/admin/users/123/streams
GET /api/v1/admin/users/123/streams?page=1&limit=25
```

**Response Format**:
```json
{
  "streams": [
    {
      "id": "b8f1c2e3-4d5a-6b7c-8d9e-0f1a2b3c4d5e",
      "starts": 1640995200,
      "ends": 1640998800,
      "state": "ended",
      "title": "My Live Stream",
      "summary": "A great streaming session",
      "image": "https://example.com/stream.jpg",
      "thumb": "https://example.com/thumb.jpg",
      "tags": ["gaming", "entertainment"],
      "content_warning": null,
      "goal": "Reach 100 viewers",
      "cost": 15000,
      "duration": 3600.5,
      "fee": 250,
      "endpoint_id": 1
    }
  ],
  "page": 0,
  "limit": 50,
  "total": 1
}
```

**Response Fields**:
- `streams`: Array of stream objects
- `page`: Current page number
- `limit`: Number of streams per page
- `total`: Total number of streams returned
- `id`: Stream UUID
- `starts`: Unix timestamp when stream started
- `ends`: Unix timestamp when stream ended (null if still live/planned)
- `state`: Stream state ("unknown", "planned", "live", "ended")
- `title`: Stream title
- `summary`: Stream description
- `image`: Stream image URL
- `thumb`: Stream thumbnail URL
- `tags`: Array of stream tags
- `content_warning`: Content warning message
- `goal`: Stream goal description
- `cost`: Total cost in millisatoshis
- `duration`: Stream duration in seconds
- `fee`: Stream fee in millisatoshis
- `endpoint_id`: ID of the ingest endpoint used

### 4. List User Balance History

**Endpoint**: `GET /api/v1/admin/users/{id}/history`

**Description**: Retrieve a paginated list of balance history for a specific user.

**Path Parameters**:
- `id`: User ID (numeric)

**Query Parameters**:
- `page` (optional, default: 0): Page number for pagination
- `limit` (optional, default: 50): Number of history entries per page

**Example Requests**:
```http
GET /api/v1/admin/users/123/history
GET /api/v1/admin/users/123/history?page=1&limit=25
```

**Response Format**:
```json
{
  "items": [
    {
      "created": 1640995200,
      "type": 0,
      "amount": 25.0,
      "desc": "Admin Credit"
    },
    {
      "created": 1640995500,
      "type": 1,
      "amount": 10.0,
      "desc": "Withdrawal"
    }
  ],
  "page": 0,
  "page_size": 50
}
```

**Response Fields**:
- `items`: Array of history entry objects
- `page`: Current page number
- `page_size`: Number of entries per page
- `created`: Unix timestamp of the transaction
- `type`: Transaction type (0 = Credit, 1 = Debit)
- `amount`: Amount in satoshis (converted from millisatoshis)
- `desc`: Description of the transaction

### 5. Get User Stream Key

**Endpoint**: `GET /api/v1/admin/users/{id}/stream-key`

**Description**: Retrieve the stream key for a specific user.

**Path Parameters**:
- `id`: User ID (numeric)

**Example Requests**:
```http
GET /api/v1/admin/users/123/stream-key
```

**Response Format**:
```json
{
  "stream_key": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

**Response Fields**:
- `stream_key`: The user's current stream key (UUID format)

### 6. Regenerate User Stream Key

**Endpoint**: `POST /api/v1/admin/users/{id}/stream-key/regenerate`

**Description**: Generate a new stream key for a specific user, replacing their current one.

**Path Parameters**:
- `id`: User ID (numeric)

**Request Body**: Empty (no body required)

**Example Requests**:
```http
POST /api/v1/admin/users/123/stream-key/regenerate
```

**Response Format**:
```json
{
  "stream_key": "f9e8d7c6-b5a4-3210-9876-543210abcdef"
}
```

**Response Fields**:
- `stream_key`: The user's new stream key (UUID format)

**Security Note**: This operation immediately invalidates the user's previous stream key. Any ongoing streams using the old key will be disconnected.

**Audit Note**: Both stream key viewing and regeneration operations are logged in the audit system for security compliance.

### 7. Get Audit Logs

**Endpoint**: `GET /api/v1/admin/audit-log`

**Description**: Retrieve a paginated list of audit logs for all admin actions. Logs are sorted by creation time in descending order (most recent first).

**Query Parameters**:
- `page` (optional, default: 0): Page number for pagination
- `limit` (optional, default: 50): Number of audit log entries per page

**Example Requests**:
```http
GET /api/v1/admin/audit-log
GET /api/v1/admin/audit-log?page=1&limit=25
```

**Response Format**:
```json
{
  "logs": [
    {
      "id": 1,
      "admin_id": 123,
      "admin_pubkey": "02a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef1234",
      "action": "grant_admin",
      "target_type": "user",
      "target_id": "456",
      "target_pubkey": "03f6e5d4c3b2a1098765432109876543210987654321fedcba0987654321fedcba",
      "message": "Admin status granted to user 456",
      "metadata": "{\"target_user_id\":456,\"admin_status\":true}",
      "created": 1640995200
    },
    {
      "id": 2,
      "admin_id": 123,
      "admin_pubkey": "02a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef1234",
      "action": "add_credit",
      "target_type": "user",
      "target_id": "456",
      "target_pubkey": "03f6e5d4c3b2a1098765432109876543210987654321fedcba0987654321fedcba",
      "message": "Added 50000 credits to user 456",
      "metadata": "{\"target_user_id\":456,\"credit_amount\":50000,\"memo\":\"Welcome bonus\"}",
      "created": 1640995100
    },
    {
      "id": 3,
      "admin_id": 123,
      "admin_pubkey": "02a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef1234",
      "action": "view_stream_key",
      "target_type": "user",
      "target_id": "456",
      "target_pubkey": "03f6e5d4c3b2a1098765432109876543210987654321fedcba0987654321fedcba",
      "message": "Admin viewed stream key for user 456",
      "metadata": "{\"target_user_id\":456}",
      "created": 1640995050
    },
    {
      "id": 4,
      "admin_id": 123,
      "admin_pubkey": "02a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef1234",
      "action": "delete_stream",
      "target_type": "stream",
      "target_id": "stream-uuid-123",
      "target_pubkey": null,
      "message": "Admin deleted stream stream-uuid-123 belonging to user 456",
      "metadata": "{\"target_stream_id\":\"stream-uuid-123\",\"target_user_id\":456,\"stream_title\":\"Stream Title\"}",
      "created": 1640995000
    }
  ],
  "page": 0,
  "limit": 50,
  "total": 4
}
```

**Response Fields**:
- `logs`: Array of audit log entry objects
- `page`: Current page number
- `limit`: Number of entries per page
- `total`: Total number of audit log entries returned
- `id`: Unique audit log entry ID
- `admin_id`: ID of the admin user who performed the action
- `admin_pubkey`: Nostr public key of the admin user (hex encoded, always present)
- `action`: Type of action performed (e.g., "grant_admin", "add_credit", "block_user")
- `target_type`: Type of resource the action was performed on (e.g., "user", "stream")
- `target_id`: ID of the target resource (string format)
- `target_pubkey`: Nostr public key of the target user (hex encoded, only present when target_type is "user")
- `message`: Human-readable description of the action
- `metadata`: JSON string containing additional structured data about the action
- `created`: Unix timestamp when the action was performed

**Action Types**:
- `grant_admin`: Admin privileges granted to a user
- `revoke_admin`: Admin privileges revoked from a user
- `block_user`: User account blocked
- `unblock_user`: User account unblocked
- `add_credit`: Credits added to user account
- `view_stream_key`: Stream key viewed by admin
- `regenerate_stream_key`: Stream key regenerated for a user
- `update_user_defaults`: User's default stream settings updated
- `delete_stream`: Stream deleted by admin

**Metadata Format**:
The metadata field contains JSON with action-specific information:
- **grant_admin/revoke_admin**: `{"target_user_id": 456, "admin_status": true}`
- **block_user/unblock_user**: `{"target_user_id": 456, "blocked_status": true}`
- **add_credit**: `{"target_user_id": 456, "credit_amount": 50000, "memo": "Welcome bonus"}`
- **view_stream_key**: `{"target_user_id": 456}`
- **regenerate_stream_key**: `{"target_user_id": 456, "new_key": "uuid-string"}`
- **update_user_defaults**: `{"target_user_id": 456, "title": "New Title", "tags": ["tag1", "tag2"]}`
- **delete_stream**: `{"target_stream_id": "stream-uuid", "target_user_id": 456, "stream_title": "Stream Title"}`

## Example Commands

First, set your Nostr secret key as an environment variable:
```bash
export NOSTR_SECRET_KEY="your-nsec-here"
```

**List Users**:
```bash
nak curl -X GET "https://api.zap.stream/api/v1/admin/users?page=0&limit=10"
```

**Grant Admin Privileges**:
```bash
nak curl -X POST "https://api.zap.stream/api/v1/admin/users/123" \
  -H "Content-Type: application/json" \
  -d '{"set_admin": true}'
```

**Add Credits**:
```bash
nak curl -X POST "https://api.zap.stream/api/v1/admin/users/123" \
  -H "Content-Type: application/json" \
  -d '{"add_credit": 50000, "memo": "Welcome bonus"}'
```

**List User Streams**:
```bash
nak curl -X GET "https://api.zap.stream/api/v1/admin/users/123/streams?page=0&limit=10"
```

**List User Balance History**:
```bash
nak curl -X GET "https://api.zap.stream/api/v1/admin/users/123/history?page=0&limit=10"
```

**Get User Stream Key**:
```bash
nak curl -X GET "https://api.zap.stream/api/v1/admin/users/123/stream-key"
```

**Regenerate User Stream Key**:
```bash
nak curl -X POST "https://api.zap.stream/api/v1/admin/users/123/stream-key/regenerate"
```

**Get Audit Logs**:
```bash
nak curl -X GET "https://api.zap.stream/api/v1/admin/audit-log?page=0&limit=10"
```