use crate::http::check_nip98_auth;
use crate::settings::Settings;
use crate::ListenerEndpoint;
use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use fedimint_tonic_lnd::tonic::codegen::Body;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response};
use nostr_sdk::{serde_json, Event, PublicKey};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::str::FromStr;
use url::Url;
use zap_stream_db::ZapStreamDb;

#[derive(Clone)]
pub struct Api {
    db: ZapStreamDb,
    settings: Settings,
}

impl Api {
    pub fn new(db: ZapStreamDb, settings: Settings) -> Self {
        Self { db, settings }
    }

    pub async fn handler(
        self,
        req: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, anyhow::Error>>, anyhow::Error> {
        let base = Response::builder()
            .header("server", "zap-stream")
            .header("access-control-allow-origin", "*")
            .header("access-control-allow-headers", "*")
            .header("access-control-allow-methods", "HEAD, GET");

        Ok(match (req.method(), req.uri().path()) {
            (&Method::GET, "/api/v1/account") => {
                let auth = check_nip98_auth(&req)?;
                let rsp = self.get_account(&auth.pubkey).await?;
                return Ok(base.body(Self::body_json(&rsp)?)?);
            }
            (&Method::PATCH, "/api/v1/account") => {
                let auth = check_nip98_auth(&req)?;
                let body = req.collect().await?.to_bytes();
                let r_body: PatchAccount = serde_json::from_slice(&body)?;
                let rsp = self.update_account(&auth.pubkey, r_body).await?;
                return Ok(base.body(Self::body_json(&rsp)?)?);
            }
            (&Method::GET, "/api/v1/topup") => {
                let auth = check_nip98_auth(&req)?;
                let url: Url = req.uri().to_string().parse()?;
                let amount: usize = url
                    .query_pairs()
                    .find_map(|(k, v)| if k == "amount" { Some(v) } else { None })
                    .and_then(|v| v.parse().ok())
                    .ok_or(anyhow!("Missing amount"))?;
                let rsp = self.topup(&auth.pubkey, amount).await?;
                return Ok(base.body(Self::body_json(&rsp)?)?);
            }
            (&Method::PATCH, "/api/v1/event") => {
                bail!("Not implemented")
            }
            (&Method::POST, "/api/v1/withdraw") => {
                bail!("Not implemented")
            }
            (&Method::POST, "/api/v1/account/forward") => {
                bail!("Not implemented")
            }
            (&Method::DELETE, "/api/v1/account/forward/<id>") => {
                bail!("Not implemented")
            }
            (&Method::GET, "/api/v1/account/history") => {
                bail!("Not implemented")
            }
            (&Method::GET, "/api/v1/account/keys") => {
                bail!("Not implemented")
            }
            _ => {
                if req.method() == Method::OPTIONS {
                    base.body(Default::default())?
                } else {
                    base.status(404).body(Default::default())?
                }
            }
        })
    }

    fn body_json<T: Serialize>(obj: &T) -> Result<BoxBody<Bytes, anyhow::Error>> {
        Ok(Full::from(serde_json::to_string(obj)?)
            .map_err(|e| match e {})
            .boxed())
    }

    async fn get_account(&self, pubkey: &PublicKey) -> Result<AccountInfo> {
        let uid = self.db.upsert_user(&pubkey.to_bytes()).await?;
        let user = self.db.get_user(uid).await?;

        Ok(AccountInfo {
            endpoints: self
                .settings
                .endpoints
                .iter()
                .filter_map(|e| match ListenerEndpoint::from_str(&e).ok()? {
                    ListenerEndpoint::SRT { endpoint } => {
                        let addr: SocketAddr = endpoint.parse().ok()?;
                        Some(Endpoint {
                            name: "SRT".to_string(),
                            url: format!("srt://{}:{}", self.settings.endpoints_public_hostname, addr.port()),
                            key: user.stream_key.clone(),
                            capabilities: vec![],
                        })
                    }
                    ListenerEndpoint::RTMP { endpoint } => {
                        let addr: SocketAddr = endpoint.parse().ok()?;
                        Some(Endpoint {
                            name: "RTMP".to_string(),
                            url: format!("rtmp://{}:{}", self.settings.endpoints_public_hostname, addr.port()),
                            key: user.stream_key.clone(),
                            capabilities: vec![],
                        })
                    }
                    ListenerEndpoint::TCP { endpoint } => {
                        let addr: SocketAddr = endpoint.parse().ok()?;
                        Some(Endpoint {
                            name: "TCP".to_string(),
                            url: format!("tcp://{}:{}", self.settings.endpoints_public_hostname, addr.port()),
                            key: user.stream_key.clone(),
                            capabilities: vec![],
                        })
                    }
                    ListenerEndpoint::File { .. } => None,
                    ListenerEndpoint::TestPattern => None,
                })
                .collect(),
            event: None,
            balance: user.balance as u64,
            tos: AccountTos {
                accepted: user.tos_accepted.is_some(),
                link: "https://zap.stream/tos".to_string(),
            },
        })
    }

    async fn update_account(&self, pubkey: &PublicKey, account: PatchAccount) -> Result<()> {
        bail!("Not implemented")
    }

    async fn topup(&self, pubkey: &PublicKey, amount: usize) -> Result<TopupResponse> {
        bail!("Not implemented")
    }
}

#[derive(Deserialize, Serialize)]
struct AccountInfo {
    pub endpoints: Vec<Endpoint>,
    pub event: Option<Event>,
    pub balance: u64,
    pub tos: AccountTos,
}

#[derive(Deserialize, Serialize)]
struct Endpoint {
    pub name: String,
    pub url: String,
    pub key: String,
    pub capabilities: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct EndpointCost {
    pub unit: String,
    pub rate: u16,
}

#[derive(Deserialize, Serialize)]
struct AccountTos {
    pub accepted: bool,
    pub link: String,
}

#[derive(Deserialize, Serialize)]
struct PatchAccount {
    pub accept_tos: Option<bool>,
}

#[derive(Deserialize, Serialize)]
struct TopupResponse {
    pub pr: String,
}
