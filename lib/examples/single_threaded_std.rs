use std::error::Error;

use r2kad_lib::domain::{NodeId, DEFAULT_ID_SIZE};

const ID_SIZE: usize = DEFAULT_ID_SIZE;

fn main() -> Result<(), Box<dyn Error>> {
    let root_id = NodeId::<ID_SIZE>::one();

    println!("Using NodeId {:#}", root_id);

    todo!("Start a Node instead of context. Node will take care of the wiring")
}
