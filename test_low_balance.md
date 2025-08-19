# Low Balance Notification Test Guide

## Configuration

Add the following to your configuration file to enable low balance notifications:

```toml
[overseer.low-balance-notification]
admin-pubkey = "02a1b2c3d4e5f6..."  # Admin's public key in hex
threshold-msats = 50000              # 50 sats threshold (50,000 millisats)
```

## How it works

1. **Low Balance Detection**: When a stream's balance falls to or below the configured threshold, the system sends notifications.

2. **User Notification**: A direct message is sent to the streaming user via Nostr with their current balance and a warning.

3. **Admin Notification**: A direct message is sent to the configured admin pubkey with user details and balance information.

4. **Spam Prevention**: Each stream only gets notified once during its session. The notification tracking is cleared when the stream ends.

## Message Format

**User Message:**
```
⚠️ Low Balance Warning ⚠️

Your streaming balance is low: X sats

Please top up your account to avoid stream interruption.
Stream ID: [stream-uuid]
```

**Admin Message:**
```
Low Balance Alert

User: [user-pubkey-hex]
Balance: X sats
Stream: [stream-uuid]

User may need assistance with topping up their account.
```

## Testing

To test the functionality:

1. Set a high threshold (e.g., 1000 sats) in the configuration
2. Create a user with a balance just above the threshold
3. Start streaming and wait for segments to process
4. The balance will decrease with each segment, triggering the notification when it falls below the threshold

## Benefits

- **Proactive Warning**: Users get advance notice before their stream is cut off
- **Admin Monitoring**: Admins can assist users who may need help topping up
- **Better User Experience**: Prevents sudden stream interruptions