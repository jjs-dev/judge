//! Allows you to send InvokeRequest's to one or several invokers.

use std::sync::Arc;

use anyhow::Context;
use invoker_api::invoke::{InvokeRequest, InvokeResponse};
use uuid::Uuid;

/// Like a database connection pool, but for invokers.
#[derive(Clone)]
pub struct Client {
    pools: Arc<[PoolInner]>,
    transport: reqwest::Client,
}

impl Client {
    /// Creates a new builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder { pools: Vec::new() }
    }

    /// Attempts to connect to a invoker instance according to the
    /// configured pools.
    pub fn instance(&self) -> anyhow::Result<Instance> {
        let pool = self.pools.first().context("no pools configured")?;
        let inst = match pool {
            PoolInner::Http { addr } => Instance {
                address: addr.clone(),
                transport: self.transport.clone(),
            },
        };
        Ok(inst)
    }
}

/// The builder for `Client`.
pub struct ClientBuilder {
    pools: Vec<PoolInner>,
}

impl ClientBuilder {
    /// Adds a new pool to the builder.
    pub fn add(&mut self, pool: Pool) {
        self.pools.push(pool.0);
    }
    /// Builds a client
    pub fn build(self) -> Client {
        Client {
            pools: self.pools.into(),
            transport: reqwest::Client::new(),
        }
    }
}

enum PoolInner {
    Http { addr: String },
}

/// A set of invokers
pub struct Pool(PoolInner);

impl Pool {
    /// Creates a pool representing invoker, listening on specified address,
    /// or several invokers behind a load-balancer. (TODO: If `single` is false,
    /// all returned instances will be one-shot.)
    pub fn new_from_address(address: &str) -> Pool {
        Pool(PoolInner::Http {
            addr: address.to_string(),
        })
    }
}

/// One invoker or several indistinguishable invokers
pub struct Instance {
    address: String,
    transport: reqwest::Client,
}

impl Instance {
    /// Sends an invokerequest
    pub async fn call(&self, mut req: InvokeRequest) -> anyhow::Result<InvokeResponse> {
        if !req.id.is_nil() {
            anyhow::bail!("request id is not nil")
        }
        req.id = Uuid::new_v4();
        let url = format!("{}/exec", self.address);
        let resp = self
            .transport
            .post(url)
            .json(&req)
            .send()
            .await
            .context("failed to send request")?
            .error_for_status()
            .context("response is not successful")?;
        let resp = resp.json().await.context("failed to receive response")?;
        Ok(resp)
    }
}
