use std::pin::Pin;
use tokio::sync::mpsc::channel;
use tonic::{transport::Server, Request, Response, Status, Streaming};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Error;

use kelly_connector::kelly_connector_server::KellyConnector;
use crate::kelly_connector::kelly_connector::{KeLLyRequest, KeLLyResponse};
use tokio_stream::{Stream, StreamExt};

pub struct GrpcServerConfig {
    pub address: String
}

pub async fn start_grpc_server(config: GrpcServerConfig) -> Result<(), Error> {

    let reflection_rename = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(kelly_connector::FILE_DESCRIPTOR_SET)
        .build()
        .unwrap();

    let test = KeLLyConnectorImpl::default();

    Server::builder()
        .add_service(reflection_rename)
        .add_service(kelly_connector::kelly_connector_server::KellyConnectorServer::new(test))
        .serve(config.address.parse().unwrap())
        .await
}

pub mod kelly_connector {
    tonic::include_proto!("kellyconnector");

    pub(crate) const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("kellyconnector");
}

#[derive(Debug, Default)]
pub struct KeLLyConnectorImpl {}

#[tonic::async_trait]
impl KellyConnector for KeLLyConnectorImpl {
    type sendRequestStream = Pin<Box<dyn Stream<Item=Result<KeLLyResponse, Status>> + Send>>;

    async fn send_request(&self, request: Request<Streaming<KeLLyRequest>>) -> Result<Response<Self::sendRequestStream>, Status> {

        let mut in_stream: Streaming<KeLLyRequest> = request.into_inner();
        let (tx, rx) = channel(128);


        let out_stream = ReceiverStream::new(rx);


        Ok(Response::new(Box::pin(out_stream) as Self::sendRequestStream))
    }
}



