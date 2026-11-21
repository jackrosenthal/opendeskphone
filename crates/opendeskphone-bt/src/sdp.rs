//! Service discovery: the gateway's two records, the server side that
//! answers a headset's queries about them, and the client query that finds
//! a headset's RFCOMM channel.  Packet handling only; the L2CAP sockets
//! (PSM 1) are in [`crate::service`].

/// Service class and protocol UUIDs (16-bit forms).
pub mod uuid {
    pub const RFCOMM: u16 = 0x0003;
    pub const L2CAP: u16 = 0x0100;
    pub const PUBLIC_BROWSE_GROUP: u16 = 0x1002;
    pub const HEADSET: u16 = 0x1108;
    pub const HEADSET_AG: u16 = 0x1112;
    pub const HANDSFREE: u16 = 0x111e;
    pub const HANDSFREE_AG: u16 = 0x111f;
    pub const GENERIC_AUDIO: u16 = 0x1203;
}

pub const PSM: u16 = 1;

mod pdu {
    pub const ERROR: u8 = 0x01;
    pub const SEARCH_REQ: u8 = 0x02;
    pub const SEARCH_RSP: u8 = 0x03;
    pub const ATTR_REQ: u8 = 0x04;
    pub const ATTR_RSP: u8 = 0x05;
    pub const SEARCH_ATTR_REQ: u8 = 0x06;
    pub const SEARCH_ATTR_RSP: u8 = 0x07;
}

const ERR_INVALID_SYNTAX: u16 = 0x0003;
const ERR_INVALID_HANDLE: u16 = 0x0002;
const ERR_INVALID_CONTINUATION: u16 = 0x0005;

const ATTR_HANDLE: u16 = 0x0000;
const ATTR_CLASS_LIST: u16 = 0x0001;
const ATTR_PROTOCOL_LIST: u16 = 0x0004;
const ATTR_BROWSE_GROUPS: u16 = 0x0005;
const ATTR_PROFILE_LIST: u16 = 0x0009;
const ATTR_NAME: u16 = 0x0100;
const ATTR_NETWORK: u16 = 0x0301;
const ATTR_FEATURES: u16 = 0x0311;

/// A data element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum De {
    Nil,
    U8(u8),
    U16(u16),
    U32(u32),
    /// A UUID on the Bluetooth base (16- and 32-bit forms, and 128-bit
    /// forms that reduce to one).
    Uuid(u32),
    /// Any other 128-bit UUID.
    Uuid128([u8; 16]),
    Text(Vec<u8>),
    Bool(bool),
    Seq(Vec<De>),
    /// A type this crate has no use for; skipped.
    Other,
}

const BASE_UUID_TAIL: [u8; 12] = [0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb];

impl De {
    pub fn uuid16(v: u16) -> De {
        De::Uuid(v as u32)
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            De::Nil | De::Other => out.push(0x00),
            De::U8(v) => out.extend_from_slice(&[0x08, *v]),
            De::U16(v) => {
                out.push(0x09);
                out.extend_from_slice(&v.to_be_bytes());
            }
            De::U32(v) => {
                out.push(0x0a);
                out.extend_from_slice(&v.to_be_bytes());
            }
            De::Uuid(v) if *v <= 0xffff => {
                out.push(0x19);
                out.extend_from_slice(&(*v as u16).to_be_bytes());
            }
            De::Uuid(v) => {
                out.push(0x1a);
                out.extend_from_slice(&v.to_be_bytes());
            }
            De::Uuid128(v) => {
                out.push(0x1c);
                out.extend_from_slice(v);
            }
            De::Text(t) => {
                // Always the one-byte length form; texts here are short.
                out.push(0x25);
                out.push(t.len().min(255) as u8);
                out.extend_from_slice(&t[..t.len().min(255)]);
            }
            De::Bool(b) => out.extend_from_slice(&[0x28, *b as u8]),
            De::Seq(items) => {
                let mut body = Vec::new();
                for i in items {
                    i.encode(&mut body);
                }
                if body.len() <= 255 {
                    out.push(0x35);
                    out.push(body.len() as u8);
                } else {
                    out.push(0x36);
                    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
                }
                out.extend_from_slice(&body);
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }

    /// Decode one element from the front of `data`; returns it and the rest.
    pub fn decode(data: &[u8]) -> Option<(De, &[u8])> {
        Self::decode_depth(data, 0)
    }

    fn decode_depth(data: &[u8], depth: u8) -> Option<(De, &[u8])> {
        if depth > 8 {
            return None;
        }
        let (&head, rest) = data.split_first()?;
        let kind = head >> 3;
        let size_index = head & 7;
        if kind == 0 {
            return Some((De::Nil, rest));
        }
        let (len, rest) = match size_index {
            0..=4 => (1usize << size_index, rest),
            5 => (*rest.first()? as usize, &rest[1..]),
            6 => (u16::from_be_bytes(rest.get(..2)?.try_into().ok()?) as usize, &rest[2..]),
            _ => (u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize, &rest[4..]),
        };
        let body = rest.get(..len)?;
        let rest = &rest[len..];
        let de = match (kind, len) {
            (1, 1) => De::U8(body[0]),
            (1, 2) => De::U16(u16::from_be_bytes(body.try_into().ok()?)),
            (1, 4) => De::U32(u32::from_be_bytes(body.try_into().ok()?)),
            (3, 2) => De::Uuid(u16::from_be_bytes(body.try_into().ok()?) as u32),
            (3, 4) => De::Uuid(u32::from_be_bytes(body.try_into().ok()?)),
            (3, 16) => {
                if body[4..] == BASE_UUID_TAIL {
                    De::Uuid(u32::from_be_bytes(body[..4].try_into().ok()?))
                } else {
                    De::Uuid128(body.try_into().ok()?)
                }
            }
            (4, _) => De::Text(body.to_vec()),
            (5, 1) => De::Bool(body[0] != 0),
            (6, _) => {
                let mut items = Vec::new();
                let mut b = body;
                while !b.is_empty() {
                    let (item, r) = Self::decode_depth(b, depth + 1)?;
                    items.push(item);
                    b = r;
                }
                De::Seq(items)
            }
            _ => De::Other,
        };
        Some((de, rest))
    }

    fn collect_uuids(&self, out: &mut Vec<De>) {
        match self {
            De::Uuid(_) | De::Uuid128(_) => out.push(self.clone()),
            De::Seq(items) => items.iter().for_each(|i| i.collect_uuids(out)),
            _ => {}
        }
    }
}

/// One service record: attributes in ascending ID order.
#[derive(Clone, Debug)]
pub struct Record {
    pub handle: u32,
    attrs: Vec<(u16, De)>,
}

impl Record {
    fn new(handle: u32, class: u16, profile: u16, version: u16, channel: u8, name: &str) -> Record {
        let attrs = vec![
            (ATTR_HANDLE, De::U32(handle)),
            (ATTR_CLASS_LIST, De::Seq(vec![De::uuid16(class), De::uuid16(uuid::GENERIC_AUDIO)])),
            (
                ATTR_PROTOCOL_LIST,
                De::Seq(vec![
                    De::Seq(vec![De::uuid16(uuid::L2CAP)]),
                    De::Seq(vec![De::uuid16(uuid::RFCOMM), De::U8(channel)]),
                ]),
            ),
            (ATTR_BROWSE_GROUPS, De::Seq(vec![De::uuid16(uuid::PUBLIC_BROWSE_GROUP)])),
            (ATTR_PROFILE_LIST, De::Seq(vec![De::Seq(vec![De::uuid16(profile), De::U16(version)])])),
            (ATTR_NAME, De::Text(name.as_bytes().to_vec())),
        ];
        Record { handle, attrs }
    }

    /// The Hands-Free audio gateway record: profile version 1.5, or with
    /// wide-band speech 1.6, the version that has it.
    pub fn handsfree_ag(channel: u8, wideband: bool) -> Record {
        let version = if wideband { 0x0106 } else { 0x0105 };
        let mut r = Record::new(0x0001_0001, uuid::HANDSFREE_AG, uuid::HANDSFREE, version, channel, "Voice Gateway");
        // Network: 1 = the gateway can reject a call.
        r.attrs.push((ATTR_NETWORK, De::U8(1)));
        // None of the optional features the attribute's first five bits
        // name (three-way calling, echo control, voice recognition, an
        // in-band ring tone, voice tags).
        r.attrs.push((ATTR_FEATURES, De::U16(if wideband { crate::hfp::SDP_WIDEBAND_SPEECH } else { 0 })));
        r
    }

    /// The Headset audio gateway record (profile version 1.2).
    pub fn headset_ag(channel: u8) -> Record {
        Record::new(0x0001_0002, uuid::HEADSET_AG, uuid::HEADSET, 0x0102, channel, "Headset Gateway")
    }

    fn matches(&self, pattern: &[De]) -> bool {
        let mut mine = Vec::new();
        for (_, v) in &self.attrs {
            v.collect_uuids(&mut mine);
        }
        !pattern.is_empty() && pattern.iter().all(|u| mine.contains(u))
    }

    /// The attribute list (a sequence of ID, value pairs) restricted to the
    /// requested IDs and ranges.
    fn attribute_list(&self, wanted: &[(u16, u16)]) -> De {
        let mut items = Vec::new();
        for (id, value) in &self.attrs {
            if wanted.iter().any(|(lo, hi)| lo <= id && id <= hi) {
                items.push(De::U16(*id));
                items.push(value.clone());
            }
        }
        De::Seq(items)
    }
}

fn be16(p: &[u8]) -> Option<(u16, &[u8])> {
    Some((u16::from_be_bytes(p.get(..2)?.try_into().ok()?), &p[2..]))
}

fn uuid_pattern(de: De) -> Option<Vec<De>> {
    match de {
        De::Seq(items) if items.iter().all(|i| matches!(i, De::Uuid(_) | De::Uuid128(_))) => Some(items),
        _ => None,
    }
}

fn id_ranges(de: De) -> Option<Vec<(u16, u16)>> {
    let De::Seq(items) = de else { return None };
    items
        .into_iter()
        .map(|i| match i {
            De::U16(id) => Some((id, id)),
            De::U32(range) => Some(((range >> 16) as u16, range as u16)),
            _ => None,
        })
        .collect()
}

/// Continuation state: absent, or our two-byte offset.
fn continuation(p: &[u8]) -> Option<usize> {
    match p {
        [0] => Some(0),
        [2, a, b] => Some(u16::from_be_bytes([*a, *b]) as usize),
        _ => None,
    }
}

fn response(id: u8, tid: u16, body: &[u8]) -> Vec<u8> {
    let mut out = vec![id];
    out.extend_from_slice(&tid.to_be_bytes());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn error(tid: u16, code: u16) -> Vec<u8> {
    response(pdu::ERROR, tid, &code.to_be_bytes())
}

/// A slice of `full` starting at `offset`, at most `max` bytes, as the body
/// of an attribute response: byte count, bytes, continuation state.
fn chunk(full: &[u8], offset: usize, max: usize) -> Option<Vec<u8>> {
    let rest = full.get(offset..)?;
    let n = rest.len().min(max.max(7));
    let mut body = (n as u16).to_be_bytes().to_vec();
    body.extend_from_slice(&rest[..n]);
    if n < rest.len() {
        body.push(2);
        body.extend_from_slice(&((offset + n) as u16).to_be_bytes());
    } else {
        body.push(0);
    }
    Some(body)
}

/// Answer one request PDU about `records`.  `mtu` is the outgoing L2CAP MTU.
/// Always returns a PDU to send, an error response for anything malformed;
/// `None` only when the request is too short to carry a transaction ID.
pub fn serve(records: &[Record], request: &[u8], mtu: usize) -> Option<Vec<u8>> {
    let id = *request.first()?;
    let (tid, rest) = be16(request.get(1..)?)?;
    let reply = (|| -> Result<Vec<u8>, u16> {
        let (plen, params) = be16(rest).ok_or(ERR_INVALID_SYNTAX)?;
        if params.len() != plen as usize {
            return Err(ERR_INVALID_SYNTAX);
        }
        // Room for the PDU header, the byte count, and the continuation.
        let room = mtu.saturating_sub(5 + 2 + 3);
        match id {
            pdu::SEARCH_REQ => {
                let (pattern, p) = De::decode(params).ok_or(ERR_INVALID_SYNTAX)?;
                let pattern = uuid_pattern(pattern).ok_or(ERR_INVALID_SYNTAX)?;
                let (max, p) = be16(p).ok_or(ERR_INVALID_SYNTAX)?;
                if continuation(p) != Some(0) {
                    return Err(ERR_INVALID_CONTINUATION);
                }
                let handles: Vec<u32> =
                    records.iter().filter(|r| r.matches(&pattern)).map(|r| r.handle).take(max as usize).collect();
                let mut body = Vec::new();
                body.extend_from_slice(&(handles.len() as u16).to_be_bytes());
                body.extend_from_slice(&(handles.len() as u16).to_be_bytes());
                for h in handles {
                    body.extend_from_slice(&h.to_be_bytes());
                }
                body.push(0);
                Ok(response(pdu::SEARCH_RSP, tid, &body))
            }
            pdu::ATTR_REQ => {
                let handle = u32::from_be_bytes(params.get(..4).ok_or(ERR_INVALID_SYNTAX)?.try_into().unwrap());
                let (max, p) = be16(&params[4..]).ok_or(ERR_INVALID_SYNTAX)?;
                let (ids, p) = De::decode(p).ok_or(ERR_INVALID_SYNTAX)?;
                let ids = id_ranges(ids).ok_or(ERR_INVALID_SYNTAX)?;
                let offset = continuation(p).ok_or(ERR_INVALID_CONTINUATION)?;
                let record = records.iter().find(|r| r.handle == handle).ok_or(ERR_INVALID_HANDLE)?;
                let full = record.attribute_list(&ids).to_bytes();
                let body = chunk(&full, offset, (max as usize).min(room)).ok_or(ERR_INVALID_CONTINUATION)?;
                Ok(response(pdu::ATTR_RSP, tid, &body))
            }
            pdu::SEARCH_ATTR_REQ => {
                let (pattern, p) = De::decode(params).ok_or(ERR_INVALID_SYNTAX)?;
                let pattern = uuid_pattern(pattern).ok_or(ERR_INVALID_SYNTAX)?;
                let (max, p) = be16(p).ok_or(ERR_INVALID_SYNTAX)?;
                let (ids, p) = De::decode(p).ok_or(ERR_INVALID_SYNTAX)?;
                let ids = id_ranges(ids).ok_or(ERR_INVALID_SYNTAX)?;
                let offset = continuation(p).ok_or(ERR_INVALID_CONTINUATION)?;
                let lists: Vec<De> =
                    records.iter().filter(|r| r.matches(&pattern)).map(|r| r.attribute_list(&ids)).collect();
                let full = De::Seq(lists).to_bytes();
                let body = chunk(&full, offset, (max as usize).min(room)).ok_or(ERR_INVALID_CONTINUATION)?;
                Ok(response(pdu::SEARCH_ATTR_RSP, tid, &body))
            }
            _ => Err(ERR_INVALID_SYNTAX),
        }
    })();
    Some(reply.unwrap_or_else(|code| error(tid, code)))
}

/// A client query for every attribute of the records of one service class.
/// `continuation` is the state from the previous response, empty at first.
pub fn search_attr_request(tid: u16, class: u16, continuation: &[u8]) -> Vec<u8> {
    let mut body = De::Seq(vec![De::uuid16(class)]).to_bytes();
    body.extend_from_slice(&0xffffu16.to_be_bytes());
    De::Seq(vec![De::U32(0x0000_ffff)]).encode(&mut body);
    body.push(continuation.len() as u8);
    body.extend_from_slice(continuation);
    response(pdu::SEARCH_ATTR_REQ, tid, &body)
}

/// The useful part of a search-attribute response: the attribute bytes it
/// carries and the continuation state (empty when complete).
pub fn parse_search_attr_response(tid: u16, packet: &[u8]) -> Option<(&[u8], &[u8])> {
    if *packet.first()? != pdu::SEARCH_ATTR_RSP {
        return None;
    }
    let (got_tid, rest) = be16(&packet[1..])?;
    let (plen, params) = be16(rest)?;
    if got_tid != tid || params.len() != plen as usize {
        return None;
    }
    let (count, rest) = be16(params)?;
    let bytes = rest.get(..count as usize)?;
    let cont = &rest[count as usize..];
    let (&cont_len, cont) = cont.split_first()?;
    if cont.len() != cont_len as usize {
        return None;
    }
    Some((bytes, cont))
}

/// The RFCOMM channel in the first record of an assembled search-attribute
/// result.
pub fn rfcomm_channel(attribute_lists: &[u8]) -> Option<u8> {
    let (De::Seq(records), _) = De::decode(attribute_lists)? else { return None };
    for record in records {
        let De::Seq(attrs) = record else { continue };
        for pair in attrs.chunks(2) {
            let [De::U16(ATTR_PROTOCOL_LIST), De::Seq(protocols)] = pair else { continue };
            for protocol in protocols {
                if let De::Seq(parts) = protocol
                    && let [De::Uuid(u), De::U8(channel), ..] = parts.as_slice()
                    && *u == uuid::RFCOMM as u32
                {
                    return Some(*channel);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> Vec<Record> {
        vec![Record::handsfree_ag(13, false), Record::headset_ag(12)]
    }

    #[test]
    fn data_elements_round_trip() {
        let de = De::Seq(vec![
            De::U8(1),
            De::U16(0x0105),
            De::U32(0x10001),
            De::uuid16(0x111f),
            De::Uuid(0x0001_0000),
            De::Text(b"Voice Gateway".to_vec()),
            De::Bool(true),
            De::Seq(vec![De::Nil]),
        ]);
        let bytes = de.to_bytes();
        assert_eq!(De::decode(&bytes), Some((de, &[][..])));
    }

    #[test]
    fn uuid128_on_the_base_reduces() {
        let mut bytes = vec![0x1c, 0x00, 0x00, 0x11, 0x1e];
        bytes.extend_from_slice(&BASE_UUID_TAIL);
        assert_eq!(De::decode(&bytes).unwrap().0, De::Uuid(0x111e));
        bytes[5] ^= 1;
        assert!(matches!(De::decode(&bytes).unwrap().0, De::Uuid128(_)));
    }

    #[test]
    fn truncated_elements_are_rejected() {
        let bytes = De::Seq(vec![De::U32(7), De::Text(b"abc".to_vec())]).to_bytes();
        for n in 1..bytes.len() {
            assert!(De::decode(&bytes[..n]).is_none(), "prefix of {n} bytes");
        }
    }

    #[test]
    fn a_headset_finds_the_handsfree_gateway_channel() {
        // The query a Hands-Free unit sends, answered by our server.
        let request = search_attr_request(7, uuid::HANDSFREE_AG, &[]);
        let reply = serve(&records(), &request, 672).unwrap();
        let (bytes, cont) = parse_search_attr_response(7, &reply).unwrap();
        assert!(cont.is_empty());
        assert_eq!(rfcomm_channel(bytes), Some(13));

        let request = search_attr_request(8, uuid::HEADSET_AG, &[]);
        let reply = serve(&records(), &request, 672).unwrap();
        let (bytes, _) = parse_search_attr_response(8, &reply).unwrap();
        assert_eq!(rfcomm_channel(bytes), Some(12));
    }

    #[test]
    fn both_records_match_generic_audio_and_none_match_a_stranger() {
        let search = |class: u16| {
            let mut body = De::Seq(vec![De::uuid16(class)]).to_bytes();
            body.extend_from_slice(&[0x00, 0x10, 0x00]);
            let reply = serve(&records(), &response(pdu::SEARCH_REQ, 1, &body), 672).unwrap();
            assert_eq!(reply[0], pdu::SEARCH_RSP);
            u16::from_be_bytes([reply[5], reply[6]])
        };
        assert_eq!(search(uuid::GENERIC_AUDIO), 2);
        assert_eq!(search(uuid::HANDSFREE_AG), 1);
        assert_eq!(search(0x110a), 0);
    }

    #[test]
    fn small_responses_continue() {
        // Ask with a 20-byte limit and follow the continuation to the end.
        let mut assembled = Vec::new();
        let mut cont: Vec<u8> = Vec::new();
        for round in 0..32 {
            let mut body = De::Seq(vec![De::uuid16(uuid::HANDSFREE_AG)]).to_bytes();
            body.extend_from_slice(&20u16.to_be_bytes());
            De::Seq(vec![De::U32(0x0000_ffff)]).encode(&mut body);
            body.push(cont.len() as u8);
            body.extend_from_slice(&cont);
            let reply = serve(&records(), &response(pdu::SEARCH_ATTR_REQ, round, &body), 672).unwrap();
            let (bytes, next) = parse_search_attr_response(round, &reply).unwrap();
            assert!(bytes.len() <= 20);
            assembled.extend_from_slice(bytes);
            cont = next.to_vec();
            if cont.is_empty() {
                break;
            }
        }
        assert!(cont.is_empty());
        assert_eq!(rfcomm_channel(&assembled), Some(13));
    }

    #[test]
    fn attribute_request_by_handle_and_id() {
        let mut body = 0x0001_0002u32.to_be_bytes().to_vec();
        body.extend_from_slice(&0xffffu16.to_be_bytes());
        De::Seq(vec![De::U16(ATTR_CLASS_LIST)]).encode(&mut body);
        body.push(0);
        let reply = serve(&records(), &response(pdu::ATTR_REQ, 3, &body), 672).unwrap();
        assert_eq!(reply[0], pdu::ATTR_RSP);
        let count = u16::from_be_bytes([reply[5], reply[6]]) as usize;
        let (list, _) = De::decode(&reply[7..7 + count]).unwrap();
        assert_eq!(
            list,
            De::Seq(vec![
                De::U16(ATTR_CLASS_LIST),
                De::Seq(vec![De::uuid16(uuid::HEADSET_AG), De::uuid16(uuid::GENERIC_AUDIO)]),
            ])
        );
    }

    #[test]
    fn malformed_requests_get_error_responses() {
        let bad_handle = {
            let mut body = 0xdeadu32.to_be_bytes().to_vec();
            body.extend_from_slice(&0xffffu16.to_be_bytes());
            De::Seq(vec![De::U32(0x0000_ffff)]).encode(&mut body);
            body.push(0);
            serve(&records(), &response(pdu::ATTR_REQ, 9, &body), 672).unwrap()
        };
        assert_eq!(bad_handle, error(9, ERR_INVALID_HANDLE));
        assert_eq!(serve(&records(), &[pdu::SEARCH_REQ, 0, 4, 0, 1, 0xff], 672).unwrap(), error(4, ERR_INVALID_SYNTAX));
        assert_eq!(serve(&records(), &[0x55, 0, 5, 0, 0], 672).unwrap(), error(5, ERR_INVALID_SYNTAX));
        assert!(serve(&records(), &[pdu::SEARCH_REQ, 0], 672).is_none());
    }
}
