use crate::settings::PaymentBackend;
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use hex::ToHex;
use lnurl::lightning_address::LightningAddress;
use lnurl::pay::PayResponse;
use lnurl::{AsyncClient, LnUrlResponse};
use nwc::NWC;
use nwc::prelude::{MakeInvoiceRequest, NostrWalletConnectURI, NotificationResult};
use payments_rs::lightning::{
    AddInvoiceRequest, AddInvoiceResponse, BitvoraNode, InvoiceUpdate, LightningNode, LndNode,
};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};
use zap_stream_db::ZapStreamDb;

pub struct NWCNode {
    conn: NWC,
}

impl NWCNode {
    pub async fn new(url: &str) -> Result<Self> {
        let url = NostrWalletConnectURI::parse(url)?;
        let conn = NWC::new(url);
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
