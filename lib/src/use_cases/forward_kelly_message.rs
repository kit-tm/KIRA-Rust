use std::marker::PhantomData;
use hex::FromHex;
use rand::thread_rng;
use crate::context::UseCaseContext;
use crate::domain::{Contact, node_id, NodeId, Path, RoutingTable, SharedPrefix};
use crate::domain::api::{KellyResponse, NodeIdApi};
use crate::messaging::{KellyReqData, KellyRspData, Nonce, ProtocolMessageSender, ReqRspMessage};
use crate::messaging::kelly_connector::KellyConnector;
use crate::messaging::source_route::SourceRoute;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ApiEvent, EventHandler, UseCaseEvent};

pub struct ForwardKellyMessageHandler<C, const BUCKET_SIZE: usize, Conn: KellyConnector> {
    context_type: PhantomData<C>,
    kelly_connector: Conn
}

impl<C, const BUCKET_SIZE: usize, Conn: KellyConnector> EventHandler for ForwardKellyMessageHandler<C, BUCKET_SIZE, Conn>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>
{
    type Context = C;
    type Error = ();
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::API(ApiEvent::SendKellyReq(request)) => self.send_initial_kelly_request(NodeIdApi { node_id: request.node_id }, request.nonce.into(),context),
            UseCaseEvent::API(ApiEvent::SendKellyRsp(route, routing_table)) => self.send_initial_kelly_response(route, routing_table, context),
            UseCaseEvent::Message(crate::messaging::ProtocolMessage::KellyReq(data), _) => self.forward_or_consume_kelly_request(data, context),
            UseCaseEvent::Message(crate::messaging::ProtocolMessage::KellyRsp(data), _) => self.consume_kelly_response(data, context),
            _ => {}
        };

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize, Conn: KellyConnector> ForwardKellyMessageHandler<C, BUCKET_SIZE, Conn>
    where
        C: UseCaseContext,
        C::Runtime: UseCaseRuntime,
        C::MessageSender: ProtocolMessageSender,
        for<'a> C::RoutingTable: crate::domain::RoutingTable<'a, BUCKET_SIZE>
{
    pub fn new(kelly_connector: Conn) -> Self {
        Self {
            context_type: PhantomData::default(),
            kelly_connector,
        }
    }

    fn send_initial_kelly_request(&self, node_id: crate::domain::api::NodeIdApi, nonce: Nonce, context: &C) {

        log::warn!("Sending Kelly message to node {:?} with hex{:?}", node_id, Vec::from_hex(node_id.node_id.clone()));
        let node_id = NodeId::from(TryInto::<[u8; node_id::SIZE]>::try_into(Vec::from_hex(node_id.node_id).unwrap()).unwrap());

        let closest_path = self.get_closest_path(&node_id, &context);
        if closest_path.is_none() {
            return
        }
        let closest_path = closest_path.unwrap();


        let mut route = SourceRoute::from(closest_path);
        route.push_front(context.root_id().clone());

        log::warn!("Sending message via route {:?}", route);

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data:  KellyReqData { node_id, nonce },
            not_via: context.not_via().clone(),
            source_route: route,
        };

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!("{}", e);
        }
    }


    fn send_initial_kelly_response(&self, source_route: Vec<crate::domain::api::NodeIdApi>, response: KellyResponse, context: &C) {

        //if rand::random::<f64>() >= 0.95_f64.powi(source_route.len() as i32) {
        //    log::error!("Dropping KeLLy Response");
        //    return;
        //}

        log::warn!("Sending Kelly response via route {:?}", source_route);

        let source_route = SourceRoute::from(source_route.into_iter().map(|x| NodeId::from(TryInto::<[u8; node_id::SIZE]>::try_into(Vec::from_hex(x.node_id).unwrap()).unwrap())).collect::<Vec<NodeId>>());

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: KellyRspData {
                response
            },
            not_via: context.not_via().clone(),
            source_route
        };

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!("{}", e);
        }

    }

    fn forward_or_consume_kelly_request(&self, mut data: ReqRspMessage<KellyReqData>, context: &C) {

        //if rand::random::<f64>() < 0.05 {
        //    log::error!("Dropping KeLLy Request");
        //    return;
        //}

        let closer_contacts = self.get_closer_contacts(&data.data.node_id, context);

        if closer_contacts.is_empty() {
            log::warn!("Consuming message, because this node is the closest node to the target");
            self.kelly_connector.forward_request(data.data.node_id, data.source_route, data.data.nonce.into());
            return;
        }

        let closest_path: Path = closer_contacts.first().map(|x| x.path()).cloned().unwrap();

        let mut new_route = data.source_route.clone();

        log::warn!("Old source route: {:?}", new_route);
        log::warn!("closest path: {:?}", closest_path);

        new_route.append(closest_path.into());
        new_route.advance();

        log::warn!("New source route: {:?}", new_route);

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: data.data,
            not_via: context.not_via().clone(),
            source_route: new_route
        };

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!("{}", e);
        }

    }

    fn consume_kelly_response(&self, data: ReqRspMessage<KellyRspData>, context: &C) {
        // we have the complete route to send the response, so we dont need to check if we need to forward it

        //log::warn!("Forwarding Response to Kelly");
        log::warn!("Hops: {}", data.source_route.ids().len() - 1);

        self.kelly_connector.forward_response(data.data.response);

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

    fn get_closer_contacts(&self, node_id: &NodeId ,context: &C) -> Vec<Contact> {

        let shared_prefix_with_root = context.root_id().shared_prefix_len(node_id, 1).unwrap().length;

        //log::info!("Received message for Id {:?}", node_id);

        let mut close: Vec<(SharedPrefix, Contact)> = context
            .routing_table()
            .closest(node_id, 1, 1)
            .expect("grouping was checked on initialization");

        let mut closer_contacts: Vec<Contact> = Vec::new();

        for contact in close {
            if shared_prefix_with_root < contact.0.length {
                log::info!("{:?} is closer than current id, {:?} vs {:?}", contact.1, contact.0.length, shared_prefix_with_root);
                closer_contacts.push(contact.1);
            } else {
                log::info!("{:?} is not closer than current id, {:?} vs {:?}", contact.1, contact.0.length, shared_prefix_with_root);
            }
        }

        closer_contacts

    }
    
}