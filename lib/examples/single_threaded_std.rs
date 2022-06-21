use std::collections::HashMap;
use std::error::Error;
use std::ops::Deref;
use std::sync::Arc;

use tokio::sync::broadcast;

use r2kad_lib::domain::{FlatRoutingTable, NodeId, DEFAULT_BUCKET_SIZE, DEFAULT_ID_SIZE};
use r2kad_lib::messaging::DummyMessageHub;
use r2kad_lib::usecases::bootstrap::{BootstrapConfig, BootstrapState, BootstrapUseCase};
use r2kad_lib::usecases::broadcaster::Broadcaster;
use r2kad_lib::usecases::{AsyncTokioContext, AsyncTokioRuntime, UseCaseEvent};

const ID_SIZE: usize = DEFAULT_ID_SIZE;

// TODO: Read from CLI
#[derive(Debug, Default)]
struct Config {
    bootstrap: BootstrapConfig,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let root_id = NodeId::<ID_SIZE>::one();

    println!("Using NodeId {:#}", root_id);

    let config = Config::default();

    let dummy_hub = DummyMessageHub::new();

    let (broadcaster, _) = broadcast::channel::<UseCaseEvent<ID_SIZE>>(100);

    let context = Arc::new(AsyncTokioContext::new(
        root_id.clone(),
        FlatRoutingTable::<ID_SIZE, DEFAULT_BUCKET_SIZE, 1>::new(root_id)?,
        HashMap::new(),
        HashMap::new(),
        dummy_hub,
        AsyncTokioRuntime::new(broadcaster.clone()),
    ));

    {
        // Bootstrap UseCase
        let mut receiver = broadcaster.subscribe();
        let context = Arc::clone(&context);
        let config = config.bootstrap.clone();
        tokio::spawn(async move {
            let mut use_case = BootstrapUseCase::new();

            if let Err(e) = use_case.start(context.deref(), &config) {
                log::error!("Failed to start Bootstrap: {}", e);
                return;
            }

            while let Ok(event) = receiver.recv().await {
                if let Err(e) = use_case.handle_event(context.deref(), &config, event) {
                    log::error!("Bootstrap failed: {}", e);
                    break;
                }

                match use_case.state() {
                    BootstrapState::Error => {
                        log::error!("Bootstrap stopped in Error state!");
                        break;
                    }
                    BootstrapState::Finished => {
                        log::debug!("Bootstrap finished!");
                        break;
                    }
                    _ => {}
                }
            }
        });
    }

    Ok(())
}
