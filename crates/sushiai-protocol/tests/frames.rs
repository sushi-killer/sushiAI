use std::fs;
use std::path::Path;

use serde_json::Value;
use sushiai_protocol::{encode, Decoder, Frame, FrameError, Message, Response, MAX_FRAME};

fn sample() -> Vec<Frame> {
    vec![
        Frame::Json(r#"{"jsonrpc":"2.0","id":1,"method":"hello"}"#.into()),
        Frame::Output {
            id: "s1".into(),
            seq: 42,
            data: vec![0, 1, 2, 0xff],
        },
        Frame::Json("{}".into()),
    ]
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn round_trip() {
    for frame in sample() {
        let mut decoder = Decoder::new();
        assert_eq!(decoder.push(&encode(&frame)).expect("decode"), vec![frame]);
    }
}

#[test]
fn split_input_byte_by_byte() {
    let bytes: Vec<u8> = sample().iter().flat_map(encode).collect();
    let mut decoder = Decoder::new();
    let mut got = Vec::new();
    for byte in bytes {
        got.extend(decoder.push(&[byte]).expect("decode"));
    }
    assert_eq!(got, sample());
}

#[test]
fn two_frames_in_one_chunk() {
    let bytes: Vec<u8> = sample().iter().take(2).flat_map(encode).collect();
    let got = Decoder::new().push(&bytes).expect("decode");
    assert_eq!(got, sample()[..2].to_vec());
}

#[test]
fn oversize_frame_is_rejected() {
    let header = ((MAX_FRAME + 1) as u32).to_be_bytes();
    assert_eq!(
        Decoder::new().push(&header),
        Err(FrameError::TooLarge(MAX_FRAME + 1))
    );
}

#[test]
fn unknown_kind_and_truncated_output_are_rejected() {
    assert_eq!(
        Decoder::new().push(&[0, 0, 0, 1, b'X']),
        Err(FrameError::UnknownKind(b'X'))
    );
    assert_eq!(
        Decoder::new().push(&[0, 0, 0, 2, b'B', 0]),
        Err(FrameError::Truncated)
    );
}

#[test]
fn golden_vectors() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors");
    let mut seen = 0;
    for entry in fs::read_dir(dir).expect("vectors dir") {
        let path = entry.expect("entry").path();
        let vector: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        let bytes = hex(vector["hex"].as_str().expect("hex"));
        let expected: Vec<Frame> = vector["frames"]
            .as_array()
            .expect("frames")
            .iter()
            .map(|f| match f["kind"].as_str().expect("kind") {
                "J" => Frame::Json(f["json"].as_str().expect("json").into()),
                _ => Frame::Output {
                    id: f["id"].as_str().expect("id").into(),
                    seq: f["seq"].as_u64().expect("seq"),
                    data: hex(f["dataHex"].as_str().expect("dataHex")),
                },
            })
            .collect();
        assert_eq!(
            Decoder::new().push(&bytes).expect("decode"),
            expected,
            "{path:?}"
        );
        let encoded: Vec<u8> = expected.iter().flat_map(encode).collect();
        assert_eq!(encoded, bytes, "{path:?}");
        seen += 1;
    }
    assert!(seen >= 4);
}

#[test]
fn messages_parse_by_shape() {
    let request =
        Message::parse(r#"{"jsonrpc":"2.0","id":3,"method":"session.list"}"#).expect("request");
    assert!(matches!(request, Message::Request(r) if r.method == "session.list"));
    let note = Message::parse(r#"{"jsonrpc":"2.0","method":"hold.exited","params":{"code":0}}"#)
        .expect("note");
    assert!(matches!(note, Message::Notification(_)));
    let reply = Response::ok(3.into(), 1);
    let Frame::Json(text) = reply.frame() else {
        panic!("json frame")
    };
    assert!(matches!(Message::parse(&text), Ok(Message::Response(r)) if r.result.is_some()));
}
