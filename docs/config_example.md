# Configuration Example for Low Balance Notifications

```toml
# Add this to your existing zap-stream configuration file

[overseer]
database = "mysql://user:pass@host/db"
relays = ["wss://relay1.example.com", "wss://relay2.example.com"]
nsec = "nsec1..."

# Enable low balance notifications
[overseer.low-balance-notification]
# Admin's public key (hex format) to receive alerts
admin-pubkey = "YOUR_ADMIN_PUBKEY_HERE"

# Balance threshold in millisats (50 sats = 50,000 millisats)
threshold-msats = 50000
```

## Public Key Format

The admin public key should be in hex format (64 characters), for example:
`02a1b2c3d4e5f6789abcdef1234567890abcdef1234567890abcdef1234567890ab`

You can get your public key in hex format from most Nostr clients or by using nostr tools.

## Threshold Examples

| Sats | Millisats | Use Case |
|------|-----------|----------|
| 10   | 10,000    | Very early warning |
| 50   | 50,000    | Recommended default |
| 100  | 100,000   | Conservative warning |
| 500  | 500,000   | High-value threshold |

Choose a threshold based on your typical streaming costs and how much advance notice users need to top up their accounts.