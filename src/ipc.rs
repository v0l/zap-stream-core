use anyhow::Error;
use async_trait::async_trait;

#[async_trait]
pub trait Rx<T> {
    async fn recv(&mut self) -> Result<T, Error>;
    fn try_recv_next(&mut self) -> Result<T, Error>;
}

#[async_trait]
impl<T> Rx<T> for tokio::sync::mpsc::UnboundedReceiver<T>
    where
        T: Send + Sync,
{
    async fn recv(&mut self) -> Result<T, Error> {
        self.recv().await.ok_or(Error::msg("recv error"))
    }

    fn try_recv_next(&mut self) -> Result<T, Error> {
        Ok(self.try_recv()?)
    }
}

#[async_trait]
impl<T> Rx<T> for tokio::sync::broadcast::Receiver<T>
    where
        T: Send + Sync + Clone,
{
    async fn recv(&mut self) -> Result<T, Error> {
        Ok(self.recv().await?)
    }
    fn try_recv_next(&mut self) -> Result<T, Error> {
        Ok(self.try_recv()?)
    }
}
