# Legacy Migration Tool

This tool migrates data from the legacy zap-stream C#/.NET system (PostgreSQL) to the current Rust-based system (MySQL). It focuses on migrating user accounts, balances, and payment history while preserving data integrity.

## Features

- **User Migration**: Converts legacy user accounts to the new system format
- **Balance Migration**: Transfers user balances accurately
- **Payment History**: Migrates payment records to maintain balance history
- **Stream Migration**: Migrates historical stream data with metadata
- **Validation**: Built-in validation to ensure data integrity
- **Dry Run Mode**: Test migration without making changes
- **Error Handling**: Robust error handling with detailed logging

## Prerequisites

- Access to both legacy PostgreSQL database and current MySQL database
- Appropriate database credentials and network access
- Rust toolchain installed

## Building

```bash
cd migration-tool
cargo build --release
```

## Usage

### Basic Migration

```bash
./target/release/legacy-migration-tool \
    --legacy-connection "postgresql://user:pass@localhost:5432/zapstream_legacy" \
    --current-connection "mysql://user:pass@localhost:3306/zapstream"
```

### Dry Run (Recommended First)

```bash
./target/release/legacy-migration-tool \
    --legacy-connection "postgresql://user:pass@localhost:5432/zapstream_legacy" \
    --current-connection "mysql://user:pass@localhost:3306/zapstream" \
    --dry-run
```

### Validation Only

```bash
./target/release/legacy-migration-tool \
    --legacy-connection "postgresql://user:pass@localhost:5432/zapstream_legacy" \
    --current-connection "mysql://user:pass@localhost:3306/zapstream" \
    --validate-only
```

### Help

```bash
./target/release/legacy-migration-tool --help
```

## Command Line Options

- `--legacy-connection CONNECTION_STRING`: Legacy PostgreSQL connection string
- `--current-connection CONNECTION_STRING`: Current MySQL connection string
- `--dry-run`: Run in dry-run mode (shows what would be migrated without making changes)
- `--validate-only`: Only run validation checks, don't perform migration

## Migration Process

The tool performs migration in the following order:

1. **User Migration**
   - Fetches users from legacy `[User]` table
   - Converts pubkeys from hex strings to binary format
   - Creates or updates users in current `user` table
   - Transfers balances and user settings

2. **Payment Migration**
   - Fetches payments from legacy `[Payment]` table
   - Maps payment types between systems
   - Creates payment records in current `payment` table
   - Maintains payment-user relationships

3. **Stream Migration**
   - Fetches streams from legacy `[Streams]` table
   - Maps stream states and metadata between systems
   - Creates stream records in current `user_stream` table
   - Preserves stream history and statistics

4. **Validation**
   - Verifies user, payment, and stream counts
   - Checks balance consistency
   - Reports any discrepancies

## Data Mapping

### Users

| Legacy Field      | Current Field     | Notes                           |
|------------------|-------------------|----------------------------------|
| PubKey           | pubkey            | Converted from hex to binary    |
| StreamKey        | stream_key        | Direct copy                     |
| Balance          | balance           | Direct copy (milli-sats)        |
| TosAccepted      | tos_accepted      | Timestamp conversion            |
| IsAdmin          | is_admin          | Direct copy                     |
| IsBlocked        | is_blocked        | Direct copy                     |
| Title            | title             | Direct copy                     |
| Summary          | summary           | Direct copy                     |
| Image            | image             | Direct copy                     |
| Tags             | tags              | Direct copy                     |
| ContentWarning   | content_warning   | Direct copy                     |
| Goal             | goal              | Direct copy                     |

### Payments

| Legacy Field     | Current Field     | Notes                           |
|------------------|-------------------|----------------------------------|
| PaymentHash      | payment_hash      | Direct copy (32-byte binary)    |
| PubKey           | user_id           | Mapped via user migration       |
| Invoice          | invoice           | Direct copy                     |
| IsPaid           | is_paid           | Direct copy                     |
| Amount           | amount            | Direct copy (milli-sats)        |
| Created          | created           | Timestamp conversion            |
| Nostr            | nostr             | Direct copy                     |
| Type             | payment_type      | Enum mapping                    |
| Fee              | fee               | Direct copy                     |

### Payment Types

| Legacy Value | Current Value | Description    |
|--------------|---------------|----------------|
| 0            | TopUp         | Account credit |
| 1            | Zap           | Lightning zap  |
| 2            | Credit        | Admin credit   |
| 3            | Withdrawal    | User withdrawal|
| 4            | AdmissionFee  | Stream fee     |

### Streams

| Legacy Field       | Current Field     | Notes                           |
|--------------------|-------------------|---------------------------------|
| Id                 | id                | GUID to string conversion       |
| PubKey             | user_id           | Mapped via user migration       |
| Starts             | starts            | Direct copy                     |
| Ends               | ends              | Direct copy                     |
| State              | state             | Enum mapping (same values)      |
| Title              | title             | Direct copy                     |
| Summary            | summary           | Direct copy                     |
| Image              | image             | Direct copy                     |
| Thumbnail          | thumb             | Field name change               |
| Tags               | tags              | Direct copy                     |
| ContentWarning     | content_warning   | Direct copy                     |
| Goal               | goal              | Direct copy                     |
| Event              | event             | Direct copy                     |
| EndpointId         | endpoint_id       | GUID to u64 mapping             |
| LastSegment        | last_segment      | Direct copy                     |
| MilliSatsCollected | cost              | decimal to u64 conversion       |
| Length             | duration          | decimal to f32 conversion       |
| AdmissionCost      | fee               | decimal to u32 conversion       |

### Stream States

| Legacy Value | Current Value | Description |
|--------------|---------------|-------------|
| 0            | Unknown       | Unknown     |
| 1            | Planned       | Planned     |
| 2            | Live          | Live        |
| 3            | Ended         | Ended       |

## Error Handling

The migration tool includes comprehensive error handling:

- **Connection Errors**: Database connectivity issues are reported clearly
- **Data Validation**: Invalid data formats are logged and skipped
- **Duplicate Detection**: Existing records are detected and handled appropriately
- **Transaction Safety**: Operations are atomic where possible

## Security Considerations

- **Database Access**: Ensure appropriate database credentials are used
- **Data Exposure**: Be cautious with connection strings containing passwords
- **Backup**: Always backup your databases before running migration
- **Validation**: Always run with `--dry-run` first to validate the process

## Troubleshooting

### Common Issues

1. **Connection Timeouts**
   - Verify network connectivity to databases
   - Check firewall rules
   - Ensure database services are running

2. **Permission Errors**
   - Verify database user has required permissions
   - Check read access on legacy database
   - Check write access on current database

3. **Data Format Errors**
   - Check pubkey format in legacy system
   - Verify payment hash lengths
   - Validate timestamp formats

### Logging

The tool provides detailed logging including:
- Migration progress with counts
- Individual user/payment processing status
- Error messages with context
- Validation results

## Support

For issues or questions regarding the migration tool:
1. Check the logs for specific error messages
2. Verify database connectivity and permissions
3. Test with `--dry-run` first
4. Contact the development team with log output if needed

## Development

To modify the migration tool:

1. Update connection logic in `MigrationTool::new()`
2. Modify data mapping in `migrate_single_user()` and `migrate_single_payment()`
3. Add validation logic in validation methods
4. Test thoroughly with dry-run mode

The tool is designed to be extensible for future migration needs.