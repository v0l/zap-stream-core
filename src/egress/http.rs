use std::net::SocketAddr;

use anyhow::Error;
use warp::{cors, Filter};

pub async fn listen_out_dir(addr: String, dir: String) -> Result<(), Error> {
    let addr: SocketAddr = addr.parse()?;
    let cors = cors().allow_any_origin().allow_methods(vec!["GET"]);

    let warp_out = warp::get().and(warp::fs::dir(dir)).with(cors);

    warp::serve(warp_out).run(addr).await;
    Ok(())
}
