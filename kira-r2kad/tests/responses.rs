// module checks if responses to received protocol messages match expecting behavior

use std::time::Instant;

use kira_r2kad::domain::{
    ConnectionId, Contact, InterfaceId, NodeId, Path, SafeStateSeqNr, UnderlayNeighborDestination,
    UnderlayNeighborId,
};
use kira_r2kad::messaging::source_route::SourceRoute;
use kira_r2kad::messaging::{Nonce, ProtocolMessage, RTableData, ReqRspMessage};
use kira_r2kad::{Input, Output, R2Kad};
use kira_r2kad::{context::SyncContext, messaging::HelloMessage};

#[test_log::test]
fn hello_response() {
    // important because deterministic_heuristic must be true to respond to Hello
    let us = NodeId::zero();
    let neighbor = NodeId::one();

    let neighbor_id = {
        let interface_id = InterfaceId::try_from(1).unwrap();
        let conn_id = ConnectionId::from(0);

        UnderlayNeighborId {
            interface_id,
            connection_id: conn_id,
        }
    };

    let mut r2kad = R2Kad::<SyncContext<_, _, _, _, _>, 20>::builder()
        .root_id(us)
        .build();
    r2kad.startup(Instant::now()).expect("successfull startup");

    // ignore set timers and initial output
    while let Some(_out) = r2kad.poll_output() {}

    // hello message
    {
        let hello_message = HelloMessage {
            source: neighbor,
            source_state_seq_nr: SafeStateSeqNr::MIN.into(),
        };
        r2kad
            .handle_input(
                Input::Message(hello_message.into(), neighbor_id),
                Instant::now(),
            )
            .expect("successfully handle HelloMessage from neighbor");

        let mut expected_response = false;
        while let Some(output) = r2kad.poll_output() {
            println!("{output:?}");
            // maybe we still fire some timers first and not only perceive a response to the request
            if matches!(output, Output::SendProtocolMessage(
                ProtocolMessage::ULNDiscReq(..),
                UnderlayNeighborDestination::UnderlayNeighbor(neighbor_dest)
            ) if neighbor_dest == neighbor_id)
            {
                expected_response = true;
                break;
            }
        }

        assert!(
            expected_response,
            "send direct ULNDiscReq to originating neighbor",
        );
    }

    // clear left over output
    while let Some(_out) = r2kad.poll_output() {}

    // uln_disc_rsp
    {
        let uln_disc_rsp = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: SafeStateSeqNr::MIN.into(),
            data: RTableData {
                contacts: vec![Contact::new(Path::from(us), SafeStateSeqNr::MIN)],
            },
            not_via: Default::default(),
            source_route: SourceRoute::new(neighbor, Path::from(us)),
        };
        let uln_disc_rsp = ProtocolMessage::ULNDiscRsp(uln_disc_rsp);

        r2kad
            .handle_input(Input::Message(uln_disc_rsp, neighbor_id), Instant::now())
            .expect("successfully handle ULNDiscRsp from neighbor");

        let mut expected_response = false;
        while let Some(output) = r2kad.poll_output() {
            // maybe we still fire some timers first and not only perceive a response to the request
            if matches!(output, Output::SendProtocolMessage(
            ProtocolMessage::UpdateRouteReq(..),
            UnderlayNeighborDestination::UnderlayNeighbor(neighbor_dest),
        ) if neighbor_dest == neighbor_id)
            {
                expected_response = true;
                break;
            }
        }

        assert!(
            expected_response,
            "send direct UpdatetRouteReq to all known neighbors",
        );
    }
}

#[test]
fn uln_disc_req_response() {
    let us = NodeId::one();

    let neighbor = NodeId::zero();
    let neighbor_id = {
        let interface_id = InterfaceId::try_from(1).unwrap();
        let conn_id = ConnectionId::from(0);

        UnderlayNeighborId {
            interface_id,
            connection_id: conn_id,
        }
    };
    let uln_disc_req = ReqRspMessage {
        nonce: Nonce::random(),
        source_state_seq_nr: SafeStateSeqNr::MIN.into(),
        data: RTableData { contacts: vec![] },
        not_via: Default::default(),
        source_route: SourceRoute::new(neighbor, Path::from(us)),
    };
    let uln_disc_req = ProtocolMessage::ULNDiscReq(uln_disc_req);

    let mut r2kad = R2Kad::<SyncContext<_, _, _, _, _>, 20>::builder()
        .root_id(us)
        .build();
    r2kad.startup(Instant::now()).expect("successfull startup");

    // ignore set timers and initial output
    while let Some(_out) = r2kad.poll_output() {}

    r2kad
        .handle_input(Input::Message(uln_disc_req, neighbor_id), Instant::now())
        .expect("successfully handle ULNDiscReq from neighbor");

    let mut discovery_dest = None;
    while let Some(output) = r2kad.poll_output() {
        if let Output::SendProtocolMessage(ProtocolMessage::ULNDiscRsp(..), dest) = output {
            discovery_dest = Some(dest);
            break;
        }
    }

    let discovery_dest = discovery_dest.expect("send UNDiscRsp to neighbor");

    assert!(
        matches!(discovery_dest, UnderlayNeighborDestination::UnderlayNeighbor(neighbor_dest) if neighbor_dest == neighbor_id),
        "send response to originating neighbor"
    );
}
