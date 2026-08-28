use std::{
    collections::{
        HashMap,
        HashSet,
    },
    io::Cursor,
    num::NonZeroU64,
    sync::Arc,
};

use kira_lib::format::ProtocolMessageFormat;
use kira_r2kad::domain::{
    Age,
    Contact,
    Link,
    NodeId,
    Path,
    ProtocolMessage,
    ProtocolMessageKind,
    SafeStateSeqNr,
    SourceRoute,
    protocol_message::{
        CommonHeader,
        ErrorData,
        FindNodeReqData,
        PathSetupReqData,
        PathTeardownReqData,
        ProbeReqData,
        ProbeRspData,
        ProtocolMessageFlags,
        QueryRouteReqData,
        QueryRouteType,
        RTableData,
        ReqRspMessage,
        RouteUpdateActionType,
        ULNReqRspMessage,
        UpdateRouteReq,
        WireFormatMessage as _,
        dht::{
            FetchReqData,
            FetchRspData,
            StoreOk,
            StoreReqData,
            StoreRspData,
        },
    },
};

#[test]
fn binrw_hello() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::ULNHello,
        NodeId::with_lsb(0x12),
        NodeId::with_lsb(0x34),
        Some(0x789.into()),
        Some(0x1234),
        1,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::ULNHello(header.clone());

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    println!("Serialized ULNHello:");

    for byte in buf.iter() {
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
        Some(0x1111.into()),
        Some(0x2222),
        2,
    );
    header.set_domain_id(0x4242);

    let contact_id = NodeId::with_lsb(0x30);
    let ssn = SafeStateSeqNr::try_from(1u32).unwrap();
    let contact = Contact::new(Path::from(contact_id), ssn);

    let req = ULNReqRspMessage {
        common_header: header.clone(),
        data: RTableData {
            contacts: vec![contact.clone()],
        },
    };

    let msg = ProtocolMessage::ULNDiscReq(req);

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    let decoded = ProtocolMessageFormat::Binrw
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

    let got = &decoded_req.data.contacts[0];
    assert_eq!(got.id(), contact.id());
    assert_eq!(got.state_seq_nr(), contact.state_seq_nr());

    assert_eq!(decoded_req.source(), src_node_id);
    assert_eq!(decoded_req.destination(), dest_node_id);
}

#[test]
fn binrw_query_route_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::QueryRouteReq,
        NodeId::with_lsb(0x40),
        NodeId::with_lsb(0x50),
        Some(0x2222.into()),
        Some(0x3333),
        3,
    );
    header.set_domain_id(0x4242);
    *header.msg_flags_mut() |= ProtocolMessageFlags::Exact;

    let req = ReqRspMessage {
        common_header: header.clone(),
        data: QueryRouteReqData {
            query_type: QueryRouteType::UnderlayNeighbors,
        },
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    };

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &ProtocolMessage::QueryRouteReq(req))
        .expect("serialize");

    println!("Serialized QueryRouteReq:");

    for byte in buf.iter() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
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
        Some(0x4444.into()),
        Some(0x5555),
        4,
    );
    header.set_domain_id(0x4242);
    *header.msg_flags_mut() |= ProtocolMessageFlags::Exact;

    let req = ReqRspMessage {
        common_header: header.clone(),
        data: FindNodeReqData {
            neighborhood: NonZeroU64::new(3).unwrap(),
        },
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    };

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &ProtocolMessage::FindNodeReq(req))
        .expect("serialize");

    println!("Serialized FindNodeReq:");

    for byte in buf.iter() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::FindNodeReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };

    assert!(
        decoded_req
            .msg_flags()
            .contains(ProtocolMessageFlags::Exact)
    );
    assert_eq!(decoded_req.data.neighborhood.get(), 3);
    assert_eq!(decoded_req.target(), header.dest_id());
    assert_eq!(decoded_req.source_route.source(), header.src_node_id());
    assert_eq!(decoded_req.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_disc_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::ULNDiscRsp,
        NodeId::with_lsb(0x71),
        NodeId::with_lsb(0x72),
        Some(0x6001.into()),
        Some(0x6002),
        5,
    );
    header.set_domain_id(0x4242);

    let contact = Contact::new(
        Path::from(NodeId::with_lsb(0x73)),
        SafeStateSeqNr::try_from(7u32).unwrap(),
    );

    let msg = ProtocolMessage::ULNDiscRsp(ULNReqRspMessage {
        common_header: header.clone(),
        data: RTableData {
            contacts: vec![contact.clone()],
        },
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized ULNDiscRsp:");

    for byte in buf.iter() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::ULNDiscRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_rsp.data.contacts[0].id(), contact.id());
    assert_eq!(
        decoded_rsp.data.contacts[0].state_seq_nr(),
        contact.state_seq_nr()
    );
    assert_eq!(decoded_rsp.source(), header.src_node_id());
    assert_eq!(decoded_rsp.destination(), header.dest_id());
}

#[test]
fn binrw_query_route_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::QueryRouteRsp,
        NodeId::with_lsb(0x81),
        NodeId::with_lsb(0x82),
        Some(0x7001.into()),
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
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized QueryRouteRsp:");

    for byte in buf.iter() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::QueryRouteRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_rsp.data.contacts[0].id(), contact.id());
    assert_eq!(
        decoded_rsp.data.contacts[0].state_seq_nr(),
        contact.state_seq_nr()
    );
}

#[test]
fn binrw_find_node_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::FindNodeRsp,
        NodeId::with_lsb(0x91),
        NodeId::with_lsb(0x92),
        Some(0x8001.into()),
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
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized FindNodeRsp:");

    for byte in buf.iter() {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::FindNodeRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_rsp.data.contacts[0].id(), contact.id());
    assert_eq!(
        decoded_rsp.data.contacts[0].state_seq_nr(),
        contact.state_seq_nr()
    );
}

#[test]
fn binrw_update_route_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::UpdateRouteReq,
        NodeId::with_lsb(0x99),
        NodeId::with_lsb(0x9a),
        Some(0x8a01.into()),
        Some(0x8a02),
        4,
    );
    header.set_domain_id(0x4242);

    let contact = Contact::new(
        Path::from(NodeId::with_lsb(0x9b)),
        SafeStateSeqNr::try_from(12u32).unwrap(),
    );

    let mut contact_actions = HashMap::new();
    contact_actions.insert(contact.clone(), RouteUpdateActionType::Announce);

    let msg = ProtocolMessage::UpdateRouteReq(UpdateRouteReq {
        common_header: header.clone(),
        not_via: Some(HashSet::new()),
        contact_actions: contact_actions.clone(),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized UpdateRouteReq:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::UpdateRouteReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };

    //Contact Timestamp/Utc made Problems, thats why we use multiple asserts instead of eq for the whole struct
    assert_eq!(decoded_req.common_header.msg_type(), header.msg_type());
    assert_eq!(decoded_req.common_header.msg_id(), header.msg_id());
    assert_eq!(
        decoded_req.common_header.state_seq_num(),
        header.state_seq_num()
    );
    assert!(decoded_req.not_via.is_some_and(|v| v.is_empty()));
    assert_eq!(decoded_req.source_route.source(), header.src_node_id());
    assert_eq!(decoded_req.source_route.destination(), header.dest_id());
    assert_eq!(decoded_req.contact_actions.len(), 1);
    let (decoded_contact, decoded_action) = decoded_req
        .contact_actions
        .iter()
        .next()
        .expect("missing contact action");
    assert_eq!(decoded_contact.id(), contact.id());
    assert_eq!(decoded_contact.state_seq_nr(), contact.state_seq_nr());
    assert_eq!(*decoded_action, RouteUpdateActionType::Announce);
}

#[test]
fn binrw_error_dead_end() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::Error,
        NodeId::with_lsb(0xa1),
        NodeId::with_lsb(0xa2),
        Some(0x9001.into()),
        Some(0x9002),
        2,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::Error(ReqRspMessage {
        common_header: header.clone(),
        data: ErrorData::DeadEnd,
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized Error(DeadEnd):");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::Error(decoded_error) = decoded else {
        panic!("unexpected message type");
    };

    assert!(matches!(decoded_error.data, ErrorData::DeadEnd));
    assert!(decoded_error.not_via.is_some_and(|v| v.is_empty()));
    assert_eq!(decoded_error.common_header.msg_type(), header.msg_type());
    assert_eq!(decoded_error.common_header.msg_id(), header.msg_id());
    assert_eq!(decoded_error.source_route.source(), header.src_node_id());
    assert_eq!(decoded_error.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_error_segment_failure() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::Error,
        NodeId::with_lsb(0xfa),
        NodeId::with_lsb(0xfb),
        Some(0xcafe.into()),
        Some(0xbabe),
        1,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::Error(ReqRspMessage {
        common_header: header.clone(),
        data: ErrorData::SegmentFailure {
            failed_link: Link::new(NodeId::with_lsb(0x42), NodeId::with_lsb(0x44)),
            source: *header.src_node_id(),
        },
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });
    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized Error message:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::Error(decoded_error) = decoded else {
        panic!("unexpected message type");
    };

    let ErrorData::SegmentFailure {
        failed_link,
        source,
    } = decoded_error.data
    else {
        panic!("unexpected ErrorData: {:?}", decoded_error.data);
    };
    assert_eq!(
        failed_link,
        Link::new(NodeId::with_lsb(0x42), NodeId::with_lsb(0x44))
    );
    assert_eq!(source, *header.src_node_id());

    assert!(decoded_error.not_via.is_some_and(|v| v.is_empty()));
    assert_eq!(decoded_error.common_header.msg_type(), header.msg_type());
    assert_eq!(decoded_error.common_header.msg_id(), header.msg_id());
    assert_eq!(decoded_error.source_route.source(), header.src_node_id());
    assert_eq!(decoded_error.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_probe_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::ProbeReq,
        NodeId::with_lsb(0xb1),
        NodeId::with_lsb(0xb2),
        Some(0x9101.into()),
        Some(0x9102),
        1,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::ProbeReq(ReqRspMessage {
        common_header: header.clone(),
        data: ProbeReqData,
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(
            *header.src_node_id(),
            Path::from([
                NodeId::random(),
                NodeId::random(),
                NodeId::random(),
                *header.dest_id(),
            ]),
        ),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized ProbeReq:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::ProbeReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_req.common_header.msg_id(), header.msg_id());
    assert_eq!(decoded_req.source_route.source(), header.src_node_id());
    assert_eq!(decoded_req.source_route.size(), 5);
    assert_eq!(decoded_req.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_probe_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::ProbeRsp,
        NodeId::with_lsb(0xb3),
        NodeId::with_lsb(0xb4),
        Some(0x9201.into()),
        Some(0x9202),
        1,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::ProbeRsp(ReqRspMessage {
        common_header: header.clone(),
        data: ProbeRspData,
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized ProbeRsp:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::ProbeRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_rsp.common_header.msg_id(), header.msg_id());
    assert_eq!(decoded_rsp.source_route.source(), header.src_node_id());
    assert_eq!(decoded_rsp.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_path_setup_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::PathSetupReq,
        NodeId::with_lsb(0xc1),
        NodeId::with_lsb(0xc2),
        Some(0x9301.into()),
        Some(0x9302),
        1,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::PathSetupReq(ReqRspMessage {
        common_header: header.clone(),
        data: PathSetupReqData,
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized PathSetupReq:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::PathSetupReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_req.common_header.msg_id(), header.msg_id());
    assert_eq!(decoded_req.source_route.source(), header.src_node_id());
    assert_eq!(decoded_req.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_path_teardown_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::PathTeardownReq,
        NodeId::with_lsb(0xc3),
        NodeId::with_lsb(0xc4),
        Some(0x9401.into()),
        Some(0x9402),
        1,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::PathTeardownReq(ReqRspMessage {
        common_header: header.clone(),
        data: PathTeardownReqData,
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized PathTeardownReq:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::PathTeardownReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };
    assert_eq!(decoded_req.common_header.msg_id(), header.msg_id());
    assert_eq!(decoded_req.source_route.source(), header.src_node_id());
    assert_eq!(decoded_req.source_route.destination(), header.dest_id());
}

#[test]
fn binrw_store_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::StoreReq,
        NodeId::with_lsb(0xd1),
        NodeId::with_lsb(0xd2),
        Some(0x9501.into()),
        Some(0x9502),
        2,
    );
    header.set_domain_id(0x4242);

    let data = Arc::from(vec![0x01u8, 0x02, 0x03]);
    let handle = NodeId::with_lsb(0xd3);

    let msg = ProtocolMessage::StoreReq(ReqRspMessage {
        common_header: header.clone(),
        data: StoreReqData {
            handle,
            data,
            last_accessed_ms: Some(Age::from(1234)),
        },
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized StoreReq:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::StoreReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };

    assert_eq!(decoded_req.common_header.msg_id(), header.msg_id());
    assert_eq!(decoded_req.data.handle, handle);
    assert_eq!(decoded_req.data.last_accessed_ms, Some(Age::from(1234)));
    assert_eq!(&*decoded_req.data.data, &[0x01u8, 0x02, 0x03]);
}

#[test]
fn binrw_store_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::StoreRsp,
        NodeId::with_lsb(0xd4),
        NodeId::with_lsb(0xd5),
        Some(0x9601.into()),
        Some(0x9602),
        2,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::StoreRsp(ReqRspMessage {
        common_header: header.clone(),
        data: StoreRspData {
            status: Ok(StoreOk::Inserted),
        },
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized StoreRsp:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::StoreRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };

    assert_eq!(decoded_rsp.common_header.msg_id(), header.msg_id());
    assert!(matches!(decoded_rsp.data.status, Ok(StoreOk::Inserted)));
}

#[test]
fn binrw_fetch_req() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::FetchReq,
        NodeId::with_lsb(0xe1),
        NodeId::with_lsb(0xe2),
        Some(0x9701.into()),
        Some(0x9702),
        2,
    );
    header.set_domain_id(0x4242);

    let handle = NodeId::with_lsb(0xe3);
    let msg = ProtocolMessage::FetchReq(ReqRspMessage {
        common_header: header.clone(),
        data: FetchReqData { handle },
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized FetchReq:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::FetchReq(decoded_req) = decoded else {
        panic!("unexpected message type");
    };

    assert_eq!(decoded_req.common_header.msg_id(), header.msg_id());
    assert_eq!(decoded_req.data.handle, handle);
}

#[test]
fn binrw_fetch_rsp() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::FetchRsp,
        NodeId::with_lsb(0xe4),
        NodeId::with_lsb(0xe5),
        Some(0x9801.into()),
        Some(0x9802),
        2,
    );
    header.set_domain_id(0x4242);

    let msg = ProtocolMessage::FetchRsp(ReqRspMessage {
        common_header: header.clone(),
        data: FetchRspData {
            data: Ok(vec![Arc::from(vec![0x0a, 0x0b]), Arc::from(vec![0x0c])]),
        },
        not_via: Some(HashSet::new()),
        source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
    });

    let mut buf = Vec::new();
    ProtocolMessageFormat::Binrw
        .serialize(&mut buf, &msg)
        .expect("serialize");

    println!("Serialized FetchRsp:");
    for byte in &buf {
        print!("{:02x} ", byte);
    }
    println!();

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize");

    let ProtocolMessage::FetchRsp(decoded_rsp) = decoded else {
        panic!("unexpected message type");
    };

    assert_eq!(decoded_rsp.common_header.msg_id(), header.msg_id());
    let Ok(values) = decoded_rsp.data.data else {
        panic!("expected fetch ok data");
    };
    assert_eq!(values.len(), 2);
    assert_eq!(&*values[0], &[0x0a, 0x0b]);
    assert_eq!(&*values[1], &[0x0c]);
}

#[test]
fn binrw_store_req_no_payload() {
    let mut header = CommonHeader::new(
        ProtocolMessageKind::StoreReq,
        NodeId::with_lsb(0xf1),
        NodeId::with_lsb(0xf2),
        Some(0xdead.into()),
        Some(0xbeef),
        1,
    );
    header.set_domain_id(0x4242);

    //set msglength equal to header length (no payload)
    header.set_msg_length(55u16);

    let mut buf = Vec::new();
    let mut cursor = binrw::io::Cursor::new(Vec::new());
    binrw::BinWrite::write_options(&header, &mut cursor, binrw::Endian::Big, ())
        .expect("write header");
    buf.extend_from_slice(cursor.get_ref());

    //deserialize should error because store-req-data missing
    let res = ProtocolMessageFormat::Binrw.deserialize(Cursor::new(&buf));
    assert!(res.is_err());
}

#[test]
fn binrw_error_unknown_error_kind() {
    use std::io::Write;
    let mut header = CommonHeader::new(
        ProtocolMessageKind::Error,
        NodeId::with_lsb(0xfa),
        NodeId::with_lsb(0xfb),
        Some(0xcafe.into()),
        Some(0xbabe),
        1,
    );
    header.set_domain_id(0x4242);

    header.set_msg_length(55u16 + 4);

    let mut buf = Vec::new();
    let mut cursor = binrw::io::Cursor::new(Vec::new());
    binrw::BinWrite::write_options(&header, &mut cursor, binrw::Endian::Big, ())
        .expect("write header");
    buf.extend_from_slice(cursor.get_ref());

    //object header
    buf.write_all(&[0x07u8, 0x00u8, 0x01u8]).unwrap();
    //unknown kind byte
    buf.write_all(&[0xffu8]).unwrap();

    let res = ProtocolMessageFormat::Binrw.deserialize(Cursor::new(&buf));
    assert!(res.is_err());
}

#[test]
fn binrw_hello_with_payload() {
    use std::io::Write;
    let mut header = CommonHeader::new(
        ProtocolMessageKind::ULNHello,
        NodeId::with_lsb(0xaa),
        NodeId::with_lsb(0xbb),
        Some(0x1234.into()),
        Some(0x5678),
        1,
    );
    header.set_domain_id(0x4242);
    header.set_msg_length(55u16 + 3);

    let mut buf = Vec::new();
    let mut cursor = binrw::io::Cursor::new(Vec::new());
    binrw::BinWrite::write_options(&header, &mut cursor, binrw::Endian::Big, ())
        .expect("write header");
    buf.extend_from_slice(cursor.get_ref());
    buf.write_all(&[0xdeu8, 0xadu8, 0xbeu8])
        .expect("write payload");

    let decoded = ProtocolMessageFormat::Binrw
        .deserialize(Cursor::new(&buf))
        .expect("deserialize hello with payload");

    assert_eq!(decoded, ProtocolMessage::ULNHello(header));
}

#[test]
///Payload/Garbage Data - should error
fn binrw_payload_only_is_error() {
    let garbage_payload = [0x07u8, 0x00u8, 0x01u8, 0xffu8, 0x10u8];
    let res = ProtocolMessageFormat::Binrw.deserialize(Cursor::new(&garbage_payload));
    assert!(res.is_err());
}
