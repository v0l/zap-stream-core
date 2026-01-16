use anyhow::{Result, anyhow, bail, ensure};
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use hex::ToHex;
use lnurl::lightning_address::LightningAddress;
use lnurl::pay::PayResponse;
use lnurl::{AsyncClient, LnUrlResponse};
use nostr_sdk::{Client, Event, EventBuilder, JsonUtil, Kind, Tag};
use nwc::prelude::{
    MakeInvoiceRequest, NostrWalletConnect, NostrWalletConnectUri, NotificationResult,
};
pub use payments_rs::lightning::LightningNode;
use payments_rs::lightning::{
    AddInvoiceRequest, AddInvoiceResponse, BitvoraNode, InvoiceUpdate, LndNode,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use zap_stream_db::ZapStreamDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentBackend {
    #[serde(rename_all = "kebab-case")]
    LND {
        address: String,
        cert: String,
        macaroon: String,
    },
    #[serde(rename_all = "kebab-case")]
    Bitvora {
        api_token: String,
        webhook_secret: String,
    },
    #[serde(rename_all = "kebab-case")]
    NWC { url: String },
    #[serde(rename_all = "kebab-case")]
    // Plain LUD-16 payment backend
    LNURL { address: String },
}

pub struct NWCNode {
    conn: NostrWalletConnect,
}

impl NWCNode {
    pub async fn new(url: &str) -> Result<Self> {
        let url = NostrWalletConnectUri::parse(url)?;
        let conn = NostrWalletConnect::new(url);
        let info = conn.get_info().await?;
        if let Some(id) = info.alias.or(info.pubkey.map(|k| k.to_string())) {
            info!("Connected to NWC node {}!", id);
        } else {
            info!("Connected to NWC!");
        }
        Ok(Self { conn })
    }
}

pub struct LNURLNode {
    address: LightningAddress,
    client: AsyncClient,
    pay_response: PayResponse,
    db: ZapStreamDb,
}

impl LNURLNode {
    pub async fn new(address: String, db: ZapStreamDb) -> Result<Self> {
        // test if the backend supports LUD-21 by generating an invoice for 1 sat
        let url = LightningAddress::new(&address)?;
        let c = AsyncClient::new();
        let rsp = c.make_request(&url.lnurlp_url()).await?;
        if let LnUrlResponse::LnUrlPayResponse(p) = rsp {
            let rsp = c.get_invoice(&p, 1000, None, None).await?;
            if rsp.verify.is_none() {
                bail!("LUD-21 not supported! Cannot use this lightning address!")
            }
            Ok(Self {
                address: url,
                pay_response: p,
                client: c,
                db,
            })
        } else {
            bail!("Invalid LNURL response!");
        }
    }
}

#[async_trait]
impl LightningNode for LNURLNode {
    async fn add_invoice(&self, req: AddInvoiceRequest) -> Result<AddInvoiceResponse> {
        let invoice = self
            .client
            .get_invoice(&self.pay_response, req.amount, None, req.memo.as_deref())
            .await?;

        Ok(AddInvoiceResponse::from_invoice(
            &invoice.pr,
            Some(invoice.verify.unwrap()),
        )?)
    }

    async fn cancel_invoice(&self, _id: &Vec<u8>) -> Result<()> {
        // not supported, ignore
        Ok(())
    }

    async fn subscribe_invoices(
        &self,
        _from_payment_hash: Option<Vec<u8>>,
    ) -> Result<Pin<Box<dyn Stream<Item = InvoiceUpdate> + Send>>> {
        // just return a task which will poll the "verify" url for incomplete payments
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        let db = self.db.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                let payments = match db.get_pending_payments().await {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Failed to load pending payments: {}", e);
                        continue;
                    }
                };
                for payment in payments {
                    if let Some(u) = payment.external_data {
                        match client.verify(&u).await {
                            Ok(r) if r.settled => {
                                if let Err(e) = tx
                                    .send(InvoiceUpdate::Settled {
                                        payment_hash: payment.payment_hash.encode_hex(),
                                        preimage: r.preimage,
                                        external_id: None,
                                    })
                                    .await
                                {
                                    warn!("Failed to send payment update: {}", e);
                                }
                            }
                            Ok(_) => {}
                            Err(e) => error!("Failed to verify payment: {}", e),
                        }
                    }
                }
            }
        });
        Ok(ReceiverStream::new(rx).boxed())
    }
}

#[async_trait]
impl LightningNode for NWCNode {
    async fn add_invoice(&self, req: AddInvoiceRequest) -> Result<AddInvoiceResponse> {
        let rsp = self
            .conn
            .make_invoice(MakeInvoiceRequest {
                amount: req.amount,
                description: req.memo,
                description_hash: None,
                expiry: req.expire.map(|r| r as _),
            })
            .await?;

        Ok(AddInvoiceResponse::from_invoice(&rsp.invoice, None)?)
    }

    async fn cancel_invoice(&self, _id: &Vec<u8>) -> Result<()> {
        // not supported, just ignore
        Ok(())
    }

    async fn subscribe_invoices(
        &self,
        _from_payment_hash: Option<Vec<u8>>,
    ) -> Result<Pin<Box<dyn Stream<Item = InvoiceUpdate> + Send>>> {
        let conn = self.conn.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        let _: JoinHandle<Result<()>> = tokio::spawn(async move {
            conn.subscribe_to_notifications().await?;
            conn.handle_notifications(|n| async {
                if let NotificationResult::PaymentReceived(i) = n.notification
                    && let Err(e) = tx
                        .send(InvoiceUpdate::Settled {
                            payment_hash: i.payment_hash,
                            preimage: Some(i.preimage),
                            external_id: None,
                        })
                        .await
                {
                    warn!("Failed to send payment update: {}", e);
                }
                Ok(false)
            })
            .await
            .map_err(|e| anyhow!("Failed to handle NWC notifications: {}", e))?;
            Ok(())
        });
        Ok(ReceiverStream::new(rx).boxed())
    }
}

/// Create the backend payment handler
pub async fn create_lightning(
    cfg: &PaymentBackend,
    db: ZapStreamDb,
) -> Result<Arc<dyn LightningNode>> {
    match cfg {
        PaymentBackend::LND {
            address,
            macaroon,
            cert,
        } => {
            info!("Using LND payment backend: {}", address);
            let lnd = LndNode::new(address, &PathBuf::from(cert), &PathBuf::from(macaroon)).await?;
            Ok(Arc::new(lnd))
        }
        PaymentBackend::Bitvora {
            api_token,
            webhook_secret,
        } => {
            info!("Using Bitvora payment backend");
            let bv = BitvoraNode::new(api_token, webhook_secret, "/api/v1/webhook/bitvora");
            Ok(Arc::new(bv))
        }
        PaymentBackend::NWC { url } => {
            info!("Using NWC payment backend");
            Ok(Arc::new(NWCNode::new(url).await?))
        }
        PaymentBackend::LNURL { address } => {
            info!("Using LNURL payment backend: {}", address);
            Ok(Arc::new(LNURLNode::new(address.clone(), db.clone()).await?))
        }
    }
}

pub struct PaymentHandler {
    lightning: Arc<dyn LightningNode>,
    db: ZapStreamDb,
    client: Client,
}

impl PaymentHandler {
    pub fn new(lightning: Arc<dyn LightningNode>, db: ZapStreamDb, client: Client) -> Self {
        Self {
            lightning,
            db,
            client,
        }
    }

    pub fn start_payment_handler(self, token: CancellationToken) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            loop {
                // get last completed payment
                let last_payment_hash = match self.db.get_latest_completed_payment().await {
                    Ok(Some(p)) => Some(p.payment_hash),
                    Ok(None) => None,
                    Err(e) => {
                        warn!("Failed to load last completed payment {}", e);
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        continue;
                    }
                };
                info!(
                    "Listening to invoices from {}",
                    last_payment_hash
                        .as_ref()
                        .map(hex::encode)
                        .unwrap_or("Now".to_string())
                );
                let mut stream = match self.lightning.subscribe_invoices(last_payment_hash).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        error!("Failed to subscribe invoices: {}", e);
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        continue;
                    }
                };
                loop {
                    tokio::select! {
                        _ = token.cancelled() => {
                            info!("Payment handler exiting...");
                            return Ok(());
                        }
                        msg = stream.next() => {
                            //info!("Received message: {:?}", msg);
                            match msg {
                               Some(InvoiceUpdate::Settled {
                                    payment_hash, preimage, ..
                                }) => {
                                    if let Err(e) = Self::try_complete_payment(payment_hash, preimage, &self.db, &self.client).await {
                                        error!("Failed to complete payment: {}", e);
                                    }
                                }
                                Some(InvoiceUpdate::Error(error)) => {
                                    error!("Invoice update error: {}", error);
                                }
                                None => {
                                    warn!("Invoice update stream ended!");
                                    continue;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        })
    }

    async fn try_complete_payment(
        payment_hash: String,
        pre_image: Option<String>,
        db: &ZapStreamDb,
        client: &Client,
    ) -> Result<()> {
        let ph = hex::decode(&payment_hash)?;
        match db.complete_payment(&ph, 0).await {
            Ok(b) => {
                if b {
                    info!("Completed payment!");
                    let payment = db.get_payment(&ph).await?.unwrap();
                    if let Some(nostr) = payment.nostr {
                        Self::try_send_zap_receipt(
                            client,
                            &nostr,
                            payment
                                .invoice
                                .ok_or(anyhow!("invoice was empty"))?
                                .as_str(),
                            pre_image,
                        )
                        .await?;
                    }
                } else {
                    warn!("No payments updated! Maybe it doesnt exist or it's already processed.")
                }
            }
            Err(e) => {
                error!("Failed to complete payment {}: {}", payment_hash, e);
            }
        }
        Ok(())
    }

    async fn try_send_zap_receipt(
        client: &Client,
        zap_request: &str,
        invoice: &str,
        pre_image: Option<String>,
    ) -> Result<()> {
        let ev = Event::from_json(zap_request)?;
        ensure!(ev.kind == Kind::ZapRequest, "Wrong zap request kind");
        ensure!(ev.verify().is_ok(), "Invalid zap request sig");

        let copy_tags = ev
            .tags
            .iter()
            .filter(|t| t.single_letter_tag().is_some())
            .cloned();
        let mut receipt = EventBuilder::new(Kind::ZapReceipt, "")
            .tags(copy_tags)
            .tag(Tag::description(zap_request))
            .tag(Tag::parse(["bolt11", invoice])?);
        if let Some(r) = pre_image {
            receipt = receipt.tag(Tag::parse(["preimage", &r])?);
        }

        let id = client.send_event_builder(receipt).await?;
        info!("Sent zap receipt {}", id.val);
        Ok(())
    }
}
