# Low Balance Notifications

This document describes the low balance notification feature that helps prevent sudden stream cut-offs by warning users before their balance runs out completely.

## Overview

The low balance notification system monitors user balances during streaming and sends proactive warnings when balances approach depletion. This gives users time to top up their accounts before their streams are interrupted.

## Configuration

Add the following configuration to your `config.toml` file:

```toml
[overseer.low-balance-notification]
admin-pubkey = "02a1b2c3d4e5f6..."  # Admin's public key in hex format
threshold-msats = 50000              # Threshold in millisats (50 sats)
```

### Configuration Parameters

- **admin-pubkey**: The public key (hex format) of an administrator who will receive low balance alerts. This allows admins to proactively reach out to users who may need assistance.
  
- **threshold-msats**: The balance threshold in millisats. When a user's balance drops to or below this amount, notifications are triggered. 
  - 1 sat = 1,000 millisats
  - Example thresholds:
    - 10,000 millisats = 10 sats
    - 50,000 millisats = 50 sats  
    - 100,000 millisats = 100 sats

## How It Works

1. **Balance Monitoring**: During stream processing, after each segment is processed and costs are deducted, the system checks if the remaining balance has fallen below the configured threshold.

2. **Notification Trigger**: When the balance drops below the threshold (but is still positive), the system sends notifications.

3. **User Notification**: An encrypted direct message is sent to the streaming user via Nostr containing:
   - Current balance in sats
   - Warning about potential stream interruption
   - Stream ID for reference

4. **Admin Notification**: An encrypted direct message is sent to the configured admin containing:
   - User's public key
   - Current balance
   - Stream ID
   - Suggestion to assist the user

5. **Spam Prevention**: Each stream session only triggers notifications once, preventing message spam if the balance continues to decrease.

## Notification Messages

### User Message Format
```
⚠️ Low Balance Warning ⚠️

Your streaming balance is low: 42 sats

Please top up your account to avoid stream interruption.
Stream ID: 123e4567-e89b-12d3-a456-426614174000
```

### Admin Message Format
```
Low Balance Alert

User: 02a1b2c3d4e5f6789abcdef1234567890abcdef1234567890abcdef1234567890ab
Balance: 42 sats
Stream: 123e4567-e89b-12d3-a456-426614174000

User may need assistance with topping up their account.
```

## Technical Implementation

### Code Changes

1. **Settings Extension**: Added `LowBalanceNotificationConfig` to the overseer configuration structure.

2. **Notification Method**: Implemented `send_low_balance_notification()` method that creates and sends encrypted direct messages via Nostr.

3. **Balance Check Enhancement**: Modified the `on_segments()` method to check for low balance before checking for zero balance.

4. **Spam Prevention**: Added tracking for notified streams using a thread-safe HashSet.

5. **Cleanup**: Added cleanup of notification tracking when streams end.

### Dependencies

The feature uses the existing `nostr-sdk` dependency for sending encrypted direct messages. No additional dependencies are required.

## Benefits

- **Proactive Communication**: Users receive advance warning before streams are cut off
- **Admin Oversight**: Administrators can monitor and assist users with balance issues
- **Better User Experience**: Reduces unexpected stream interruptions
- **Customer Support**: Enables proactive customer support for users experiencing balance issues

## Testing

To test the low balance notification feature:

1. **Configure High Threshold**: Set a high threshold (e.g., 1000 sats) in your test environment
2. **Create Test User**: Create a user with a balance slightly above the threshold
3. **Start Streaming**: Begin a stream which will consume balance with each segment
4. **Monitor Notifications**: Watch for notifications when the balance drops below the threshold
5. **Verify Messages**: Check that both user and admin receive appropriate notifications

## Migration

This feature is backward compatible. Existing configurations will continue to work without the low balance notification feature. To enable it, simply add the configuration section to your existing config file.

## Security Considerations

- **Private Keys**: Admin public keys should be carefully managed and verified
- **Message Privacy**: All notifications are sent as encrypted direct messages via Nostr
- **Rate Limiting**: The spam prevention mechanism ensures users aren't overwhelmed with notifications

## Troubleshooting

### Common Issues

1. **No Notifications Received**: 
   - Verify admin public key is correct hex format
   - Check that threshold is appropriately set
   - Ensure Nostr relays are connected

2. **Multiple Notifications**: 
   - This shouldn't happen due to spam prevention, but could indicate stream restart issues

3. **Configuration Errors**:
   - Verify TOML syntax is correct
   - Ensure public key is valid hex format (64 characters)
   - Check that threshold is a positive integer

### Logs

The system logs notification attempts with the following messages:
- `Sent low balance notification to user {user_id}`
- `Sent low balance alert to admin for user {user_id}`
- `Failed to send low balance notification to user {user_id}: {error}`
- `Failed to send low balance alert to admin for user {user_id}: {error}`