use std::collections::HashMap;
use std::error::Error;
use std::ops::Deref;
use std::sync::Arc;

use tokio::runtime::Runtime;
use tokio::sync::broadcast;

use r2kad_lib::domain::{FlatRoutingTable, NodeId, DEFAULT_BUCKET_SIZE, DEFAULT_ID_SIZE};
use r2kad_lib::messaging::DummyMessageHub;
use r2kad_lib::usecases::bootstrap::{BootstrapConfig, BootstrapState, BootstrapUseCase};
use r2kad_lib::usecases::{AsyncTokioContext, AsyncTokioRuntime, UseCaseEvent};

const ID_SIZE: usize = DEFAULT_ID_SIZE;

// TODO: Read from CLI
#[derive(Debug, Default)]
struct Config {
    bootstrap: BootstrapConfig,
}

fn main() -> Result<(), Box<dyn Error>> {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );

    env_logger::init();

    let root_id: NodeId<ID_SIZE> = std::env::var("NODE_ID")?
        .parse()
        .unwrap_or_else(|_| NodeId::random());

    println!("Using NodeId {}", root_id);

    let config = Config::default();

    let (broadcaster, mut receiver) = broadcast::channel::<UseCaseEvent<ID_SIZE>>(100);

    let context = Arc::new(AsyncTokioContext::new(
        root_id.clone(),
        FlatRoutingTable::<ID_SIZE, DEFAULT_BUCKET_SIZE, 1>::new(root_id)?,
        HashMap::new(),
        HashMap::new(),
        DummyMessageHub::new(),
        AsyncTokioRuntime::new(broadcaster.clone(), Arc::clone(&runtime)),
    ));

    let mut use_case = BootstrapUseCase::new();
    use_case.start(context.deref(), &config.bootstrap)?;

    // TODO:
    //  Start MessageReceivers and wait for message from broadcaster or MessageReceivers
    //  to delegate to use cases.

    while let Ok(event) = runtime.block_on(receiver.recv()) {
        if let Err(e) = use_case.handle_event(context.deref(), &config.bootstrap, event) {
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

    Ok(())
}
