pub mod admin_api;
pub mod api_base;
pub mod http;
pub mod multitrack;
pub mod nostr;
pub mod payments;
pub mod plugin;
pub mod stream_manager;
pub mod viewer;

pub use payments_rs::lightning::setup_crypto_provider;
use zap_stream_api_common::HistoryEntry;
use zap_stream_db::UserHistoryEntry;

pub fn user_history_to_api_model(entry: UserHistoryEntry) -> HistoryEntry {
    let (entry_type, desc) = if let Some(payment_type) = entry.payment_type {
        // This is a payment entry
        let entry_type = match payment_type {
            3 => 1, // Withdrawal = Debit (PaymentType::Withdrawal = 3)
            _ => 0, // Credit (TopUp, Zap, Credit, AdmissionFee)
        };
        let desc = match payment_type {
            3 => Some("Withdrawal".to_string()), // PaymentType::Withdrawal = 3
            2 => Some("Admin Credit".to_string()), // PaymentType::Credit = 2
            1 => entry.nostr,                    // PaymentType::Zap = 1, use nostr content
            0 => Some("Topup".to_string()),
            _ => None,
        };
        (entry_type, desc)
    } else {
        // This is a stream entry
        let desc = Some(format!(
            "Stream: {}",
            entry
                .stream_title
                .unwrap_or_else(|| entry.stream_id.unwrap_or_else(|| "Unknown".to_string()))
        ));
        (1, desc) // Debit
    };

    HistoryEntry {
        created: entry.created.timestamp() as u64,
        entry_type,
        amount: entry.amount as f64 / 1000.0, // Convert from milli-sats to sats
        desc,
    }
}
