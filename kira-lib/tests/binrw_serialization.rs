use kira_lib::format::ProtocolMessageFormat;
use kira_r2kad::domain::{Contact, NodeId, Path, SafeStateSeqNr};
use kira_r2kad::messaging::source_route::SourceRoute;
use kira_r2kad::messaging::{
    CommonHeader, FindNodeReqData, KiraMsgFlagsBit, ProtocolMessage, ProtocolMessageKind,
    QueryRouteReqData, QueryRouteType, RTableData, ReqRspMessage,
};
use std::collections::HashSet;
use std::io::Cursor;
use std::num::NonZeroU64;

#[test]
fn binrw_hello() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::ULNHello,
        NodeId::with_lsb(0x12),
        NodeId::with_lsb(0x34),
        Some(0x789),
        Some(0x1234),
        1,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::ULNHello(header.clone());

    let mut buf = Vec::new();
    ProtocolMessageFormat::BINRW
        .serialize(&mut buf, &msg)
        .expect("serialize");

    let decoded = ProtocolMessageFormat::BINRW
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    println!("Serialized ULNHello:");

    for (_i, byte) in buf.iter().enumerate() {
        print!("{:02x} ", byte);
    }
    println!();

    assert_eq!(decoded, ProtocolMessage::ULNHello(header));
}

#[test]
fn binrw_discreq() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::ULNDiscReq,
        NodeId::with_lsb(0x10),
        NodeId::with_lsb(0x20),
        Some(0x1111),
        Some(0x2222),
        2,
    );
    header.set_domain_id(0x4242);

    let contact_id = NodeId::with_lsb(0x30);
    let ssn = SafeStateSeqNr::try_from(1u32).unwrap();
    let contact = Contact::new(Path::from(contact_id), ssn);

    let req = ReqRspMessage {
        common_header: header.clone(),
        data: RTableData {
            contacts: vec![contact.clone()],
        },
        not_via: HashSet::new(),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    };

    let msg = ProtocolMessage::ULNDiscReq(req);

    let mut buf = Vec::new();
    ProtocolMessageFormat::BINRW
        .serialize(&mut buf, &msg)
        .expect("serialize");

    let decoded = ProtocolMessageFormat::BINRW
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    println!("Serialized ULNDiscReq:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let src_node_id = header.src_node_id();
    let dest_node_id = header.dest_id();

    let ProtocolMessage::ULNDiscReq(decoded_req) = &decoded else {
        panic!("unexpected message: {decoded:?}");
    };

    let hdr = &decoded_req.common_header;
    assert_eq!(hdr.msg_type(), header.msg_type());
    assert_eq!(hdr.dest_id(), header.dest_id());
    assert_eq!(hdr.src_node_id(), header.src_node_id());
    assert_eq!(hdr.domain_id(), header.domain_id());
    assert_eq!(hdr.msg_id(), header.msg_id());
    assert_eq!(hdr.state_seq_num(), header.state_seq_num());
    assert_eq!(hdr.src_node_degree(), header.src_node_degree());
    assert!(hdr.msg_length() >= 55u16);

    assert!(decoded_req.not_via.is_empty());

    let got = &decoded_req.data.contacts[0];
    assert_eq!(got.id(), contact.id());
    assert_eq!(got.state_seq_nr(), contact.state_seq_nr());

    assert_eq!(decoded_req.source_route.source(), src_node_id);
    assert_eq!(decoded_req.source_route.destination(), dest_node_id);
}

#[test]
fn binrw_query_route_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::QueryRouteReq,
        NodeId::with_lsb(0x40),
        NodeId::with_lsb(0x50),
        Some(0x2222),
        Some(0x3333),
        3,
    );
    header.set_domain_id(0x4242);
    header.set_flag(KiraMsgFlagsBit::ExactFlag);

    let req = ReqRspMessage {
        common_header: header.clone(),
        data: QueryRouteReqData {
            query_type: QueryRouteType::UnderlayNeighbors,
        },
        not_via: HashSet::new(),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    };

    let mut buf = Vec::new();
    ProtocolMessageFormat::BINRW
        .serialize(&mut buf, &ProtocolMessage::QueryRouteReq(req))
        .expect("serialize");

    println!("Serialized QueryRouteReq:");

    for (_i, byte) in buf.iter().enumerate() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::BINRW
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::QueryRouteReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };

    assert!(matches!(
        decoded_req.data.query_type,
        QueryRouteType::UnderlayNeighbors
    ));
    assert_eq!(decoded_req.common_header.msg_type(), header.msg_type());
    assert_eq!(decoded_req.source_route.source(), header.src_node_id());
    assert_eq!(decoded_req.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_find_node_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::FindNodeReq,
        NodeId::with_lsb(0x60),
        NodeId::with_lsb(0x70),
        Some(0x4444),
        Some(0x5555),
        4,
    );
    header.set_domain_id(0x4242);
    header.set_flag(KiraMsgFlagsBit::ExactFlag);

    let req = ReqRspMessage {
        common_header: header.clone(),
        data: FindNodeReqData {
            exact: true,
            neighborhood: NonZeroU64::new(3).unwrap(),
            target: *header.dest_id(),
        },
        not_via: HashSet::new(),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    };

    let mut buf = Vec::new();
    ProtocolMessageFormat::BINRW
        .serialize(&mut buf, &ProtocolMessage::FindNodeReq(req))
        .expect("serialize");

    println!("Serialized FindNodeReq:");

    for (_i, byte) in buf.iter().enumerate() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::BINRW
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::FindNodeReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };

    assert!(decoded_req.data.exact);
    assert_eq!(decoded_req.data.neighborhood.get(), 3);
    assert_eq!(decoded_req.data.target, *header.dest_id());
    assert_eq!(decoded_req.source_route.source(), header.src_node_id());
    assert_eq!(decoded_req.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_disc_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::ULNDiscRsp,
        NodeId::with_lsb(0x71),
        NodeId::with_lsb(0x72),
        Some(0x6001),
        Some(0x6002),
        5,
    );
    header.set_domain_id(0x4242);

    let contact = Contact::new(
        Path::from(NodeId::with_lsb(0x73)),
        SafeStateSeqNr::try_from(7u32).unwrap(),
    );

    let msg = ProtocolMessage::ULNDiscRsp(ReqRspMessage {
        common_header: header.clone(),
        data: RTableData {
            contacts: vec![contact.clone()],
        },
        not_via: HashSet::new(),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::BINRW
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized ULNDiscRsp:");

    for (_i, byte) in buf.iter().enumerate() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::BINRW
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::ULNDiscRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_rsp.data.contacts[0].id(), contact.id());
    assert_eq!(decoded_rsp.data.contacts[0].state_seq_nr(), contact.state_seq_nr());
}

#[test]
fn binrw_query_route_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::QueryRouteRsp,
        NodeId::with_lsb(0x81),
        NodeId::with_lsb(0x82),
        Some(0x7001),
        Some(0x7002),
        6,
    );
    header.set_domain_id(0x4242);

    let contact = Contact::new(
        Path::from(NodeId::with_lsb(0x83)),
        SafeStateSeqNr::try_from(8u32).unwrap(),
    );

    let msg = ProtocolMessage::QueryRouteRsp(ReqRspMessage {
        common_header: header.clone(),
        data: RTableData {
            contacts: vec![contact.clone()],
        },
        not_via: HashSet::new(),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::BINRW
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized QueryRouteRsp:");

    for (_i, byte) in buf.iter().enumerate() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::BINRW
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::QueryRouteRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_rsp.data.contacts[0].id(), contact.id());
    assert_eq!(decoded_rsp.data.contacts[0].state_seq_nr(), contact.state_seq_nr());
}

#[test]
fn binrw_find_node_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::FindNodeRsp,
        NodeId::with_lsb(0x91),
        NodeId::with_lsb(0x92),
        Some(0x8001),
        Some(0x8002),
        7,
    );
    header.set_domain_id(0x4242);

    let contact = Contact::new(
        Path::from(NodeId::with_lsb(0x93)),
        SafeStateSeqNr::try_from(9u32).unwrap(),
    );

    let msg = ProtocolMessage::FindNodeRsp(ReqRspMessage {
        common_header: header.clone(),
        data: RTableData {
            contacts: vec![contact.clone()],
        },
        not_via: HashSet::new(),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::BINRW
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized FindNodeRsp:");

    for (_i, byte) in buf.iter().enumerate() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::BINRW
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::FindNodeRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_rsp.data.contacts[0].id(), contact.id());
    assert_eq!(decoded_rsp.data.contacts[0].state_seq_nr(), contact.state_seq_nr());
}
