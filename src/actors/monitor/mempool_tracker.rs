use std::{collections::HashMap, sync::Arc};

use actix::Addr;

use crate::{
    actors::{PauseProducer, Producer, ResumeProducer},
    eth::{EthHttpCli, MempoolStatus},
};

pub(crate) struct MempoolTracker {
    max_pool_size: usize,
}

impl MempoolTracker {
    pub fn new(max_pool_size: usize) -> Self {
        Self { max_pool_size }
    }

    pub async fn get_pool_status(
        clients: &HashMap<Arc<String>, Arc<EthHttpCli>>,
    ) -> Vec<anyhow::Result<MempoolStatus>> {
        let mut statuses = Vec::new();
        for client in clients.values() {
            let pool_status = client.get_mempool_status().await;
            statuses.push(pool_status);
        }
        statuses
    }

    pub fn process_pool_status(
        &self,
        status: Vec<anyhow::Result<MempoolStatus>>,
        producer_addr: &Addr<Producer>,
    ) -> Result<(u64, u64), anyhow::Error> {
        let _ = producer_addr;
        let mut total_pending = 0;
        let mut total_queued = 0;
        for status in status.into_iter().flatten() {
            total_pending += status.pending;
            total_queued += status.queued;
        }
        if total_pending + total_queued > self.max_pool_size {
            producer_addr.do_send(PauseProducer);
        } else if total_pending + total_queued < self.max_pool_size / 2 {
            producer_addr.do_send(ResumeProducer);
        }
        Ok((total_pending as u64, total_queued as u64))
    }
}
