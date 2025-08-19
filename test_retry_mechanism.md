# Testing Stream Event Retry Mechanism

## Manual Testing Instructions

To test the retry mechanism for stream event publishing, you can:

### 1. Network Simulation Testing
- Temporarily configure invalid or slow Nostr relays in the configuration
- Attempt to update stream metadata via the API
- Observe logs for retry attempts with exponential backoff (1s, 2s, 4s delays)
- Verify that database operations complete successfully even if event publishing fails

### 2. Relay Rate Limiting Testing  
- Configure relays known to have rate limits
- Rapidly update stream metadata multiple times
- Observe retry behavior when rate limits are hit
- Verify streams continue to function normally

### 3. Expected Log Output
When retries occur, you should see logs like:
```
WARN Failed to publish event <event_id> (attempt 1/4), retrying in 1000ms: <error>
WARN Failed to publish event <event_id> (attempt 2/4), retrying in 2000ms: <error>
WARN Failed to publish event <event_id> (attempt 3/4), retrying in 4000ms: <error>
```

On success after retries:
```
INFO Successfully published event <event_id> after 2 retries
```

### 4. Graceful Degradation Verification
- Verify that streams can start even if initial event publishing fails
- Verify that streams can end properly even if final event publishing fails  
- Verify that viewer count updates continue working even if some publish attempts fail
- Verify that database state remains consistent regardless of event publishing results

## Unit Tests
The implementation includes unit tests for:
- Retry configuration validation
- Exponential backoff delay calculation
- Basic retry mechanism logic

Run with: `cargo test overseer::tests`