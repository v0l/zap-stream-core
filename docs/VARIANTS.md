# Variant System - Admin Guide

The variant system enables adaptive bitrate streaming by creating multiple quality levels from a single input stream. This guide covers configuration, billing, and automatic quality management for backend administrators.

## Overview

Variants allow viewers to receive the best possible quality based on their network conditions. The system automatically prevents upscaling (creating variants larger than the source) and bills users in real-time based on usage.

## Variant Configuration

### Ingest Endpoint Table

Variants are configured in the `ingest_endpoint` database table with three key fields:

- **name**: Human-readable endpoint identifier
- **cost**: Cost per minute in millisatoshis (1 sat = 1,000 msat)
- **capabilities**: Comma-separated variant configuration string

### Capability String Format

The capabilities field defines what variants and features are available for each endpoint:

```
variant:source,variant:720:2500000,variant:480:1200000,dvr:720
```

**Capability Types:**

- `variant:source` - Pass-through the original stream without transcoding
- `variant:{height}:{bitrate}` - Create transcoded variant at specific resolution and bitrate
- `dvr:{height}` - Enable recording capability for specific resolution

**Common Configurations:**

- **Basic**: `variant:source` (source quality only, no transcoding)
- **Standard**: `variant:source,variant:720:2500000,variant:480:1200000`
- **Premium**: `variant:source,variant:1080:5000000,variant:720:2500000,variant:480:1200000,dvr:1080`

### Cost Structure

Endpoint costs are set per minute in millisatoshis:

- **5,000 msat/min** (5 sats/min) - Basic streaming, source only
- **10,000 msat/min** (10 sats/min) - Standard with 2-3 variants
- **15,000-20,000 msat/min** (15-20 sats/min) - Premium with recording

## Automatic Upscaling Prevention

The system automatically prevents creating variants larger than the source stream to avoid quality degradation and wasted resources.

**How it works:**
- Source resolution is detected when stream starts
- Any configured variants larger than source are automatically disabled
- Only variants equal to or smaller than source are created

**Examples:**
- **480p source** → Available: source, 480p (720p and 1080p disabled)
- **720p source** → Available: source, 720p, 480p (1080p disabled)
- **1080p source** → Available: source, 1080p, 720p, 480p (all enabled)

**Benefits:**
- No CPU waste on upscaling
- Prevents quality degradation
- Automatic cost optimization
- No manual intervention required

## Billing System

### Real-Time Billing

Users are billed automatically during streaming:

- Billing occurs per HLS segment (typically every 2-6 seconds)
- Cost calculated based on segment duration × endpoint cost per minute
- User balance decremented in real-time
- Stream automatically stops when balance reaches zero

### Balance Management

User balances are stored in millisatoshis in the user table:

- Default balance: 0 msat
- Balances can be positive (credits) or zero (no streaming allowed)
- No overdraft permitted - streams stop immediately at zero balance

### Admin Credit Management

Admins can manage user balances through the Admin API:

**Add Credits:**
```json
POST /api/v1/admin/users/{user_id}
{
  "add_credit": 25000,
  "memo": "Monthly credit allocation"
}
```

**User Management:**
```json
POST /api/v1/admin/users/{user_id}
{
  "set_admin": true,
  "set_blocked": false
}
```

### Payment Types

The system tracks different payment types:
- **Credit**: Admin-added credits
- **Debit**: Stream usage costs
- **TopUp**: User Lightning payments
- **Withdrawal**: User withdrawals
- **AdmissionFee**: Platform fees

## Cost Optimization

### Variant Impact on Resources

- **Source only**: Minimal CPU usage, lowest cost
- **Single transcode**: ~100% CPU per stream
- **Multiple variants**: ~300% CPU for 3 variants
- **DVR recording**: Additional storage costs

### Recommended Configurations

**Budget Endpoints:**
- Capabilities: `variant:source`
- Cost: 5,000 msat/min
- Use case: Simple restreaming

**Standard Endpoints:**
- Capabilities: `variant:source,variant:720:2500000,variant:480:1200000`
- Cost: 10,000 msat/min
- Use case: Adaptive streaming

**Premium Endpoints:**
- Capabilities: `variant:source,variant:1080:5000000,variant:720:2500000,variant:480:1200000,dvr:1080`
- Cost: 20,000 msat/min
- Use case: Professional streaming with recording

## Admin Monitoring

### Key Metrics to Track

- Active streams per endpoint type
- CPU utilization by variant configuration
- User balance distribution
- Stream terminations due to insufficient funds
- Upscaling prevention events

### Financial Monitoring

Track revenue and usage patterns:
- Daily/monthly revenue by endpoint
- Average stream duration by endpoint cost
- User balance depletion rates
- Most popular variant configurations

### User Management

Monitor user behavior:
- Users with zero balances
- High-usage users requiring credit top-ups
- Blocked or problematic accounts
- Admin accounts and permissions

## Troubleshooting

### Common Issues

**Variants Not Creating:**
- Check if source resolution is smaller than configured variants
- Verify capabilities string format
- Ensure endpoint exists in database

**Streams Stopping Unexpectedly:**
- Check user balance - streams stop at zero balance
- Verify user is not blocked
- Check for encoding errors in logs

**High CPU Usage:**
- Review variant configurations - multiple transcodes are CPU-intensive
- Consider reducing number of variants for high-traffic endpoints
- Check for hardware acceleration availability

### Log Monitoring

Key log events to monitor:
- "Skipping variant" - Indicates upscaling prevention
- "Insufficient balance" - User balance issues
- "Creating variants" - Successful variant setup
- Encoding errors or failures

This system provides automatic quality management while ensuring fair billing and resource optimization for your streaming platform.