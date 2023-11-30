use std::marker::PhantomData;
use hex::FromHex;
use crate::context::UseCaseContext;
use crate::domain::{node_id, NodeId, Path, RoutingTable};
use crate::messaging::{KellyReqData, KellyRspData, Nonce, ProtocolMessageSender, ReqRspMessage};
use crate::messaging::source_route::SourceRoute;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ApiEvent, EventHandler, UseCaseEvent};

pub struct ForwardKellyMessageHandler<C, const BUCKET_SIZE: usize> {
    context_type: PhantomData<C>
}

impl<C, const BUCKET_SIZE: usize> Default for ForwardKellyMessageHandler<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            context_type: PhantomData::default()
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for ForwardKellyMessageHandler<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: crate::domain::RoutingTable<'a, BUCKET_SIZE>
{
    type Context = C;
    type Error = ();
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::API(ApiEvent::SendKellyReq(node_id)) => self.send_initial_kelly_message(node_id, context),
            UseCaseEvent::API(ApiEvent::SendKellyRsp()) => {},
            UseCaseEvent::Message(crate::messaging::ProtocolMessage::KellyReq(data), _) => self.forward_or_consume_kelly_request(data, context),
            UseCaseEvent::Message(crate::messaging::ProtocolMessage::KellyRsp(data), _) => self.forward_or_consume_kelly_response(data, context),
            _ => None
        };

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> ForwardKellyMessageHandler<C, BUCKET_SIZE>
    where
        C: UseCaseContext,
        C::Runtime: UseCaseRuntime,
        C::MessageSender: ProtocolMessageSender,
        for<'a> C::RoutingTable: crate::domain::RoutingTable<'a, BUCKET_SIZE>
{

    async fn forward_or_consume_kelly_request(&self, mut data: ReqRspMessage<KellyReqData>, context: C) {
        

        let closer_contacts = context
            .routing_table()
            .closest(&data.data.node_id, BUCKET_SIZE, 1)
            .expect("grouping was checked on initialization");

        if closer_contacts.is_empty() {
            // we assume we are the closest contact and call the kelly node
            let client = reqwest::Client::new();
            client.post("0.0.0.0:8081")
                .body(crate::domain::api::NodeId::from(data.data.node_id))
                .send()
                .await
                .expect("TODO: panic message");
            return;
        }

        let closest_path: Path = closer_contacts.first().map(|(_, x)| x.path()).cloned().unwrap();

        let mut new_route = data.source_route.clone();

        new_route.append(closest_path.into());
        new_route.advance();
                
        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data,
            not_via: context.not_via().clone(),
            source_route: new_route
        };

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!("{}", e);
        }

    }

    fn forward_or_consume_kelly_response(&self, data: ReqRspMessage<KellyRspData>, context: C) {

    }

    fn send_initial_kelly_message(&self, node_id: crate::domain::api::NodeId, context: C) {

        log::info!("Sending Kelly message to node {:?}", node);
        let bytes = Vec::from_hex(node_id.node_id).unwrap();

        let node : [u8; node_id::SIZE] = bytes.try_into().unwrap();


        let closest_path = self.get_closest_path(&NodeId::from(node), &context);
        if closest_path.is_none() {
            return
        }
        let closest_path = closest_path.unwrap();


        let mut route = SourceRoute::from(closest_path);
        route.push_front(context.root_id().clone());

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: KellyReqData { node_id: context.root_id().clone() },
            not_via: context.not_via().clone(),
            source_route: route,
        };

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!("{}", e);
        }
    }

    fn get_closest_path(&self, node: &NodeId, context: &C) -> Option<Path>{
        let closest_path = context
            .routing_table()
            .closest(
                &node,
                BUCKET_SIZE,
                1, // FIXME check what needs to be put here
            )
            .expect("grouping was checked on initialization")
            .first()
            .map(|(_, contact)| contact.path())
            .cloned();
        if closest_path.is_none() {
            log::trace!(
                target: "forward kelly message",
                "No closest contact found for random id; Assuming isolation"
            );
        }

        closest_path
    }

    fn send_initial_kelly_response(&self, context: C) {

    }
    
}