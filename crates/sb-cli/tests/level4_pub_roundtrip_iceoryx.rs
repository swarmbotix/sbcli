//! L4 TDD #8 (iox2 half) — `sb topic pub --transport iceoryx2` round-trips
//! bytes through an iox2 `[u8]`-slab service.
//!
//! Pre-arms an iox2 subscriber in a thread, runs the CLI to publish
//! `deadbeef` on the same service, and verifies the subscriber sees the
//! exact 4 bytes. The subscriber waits for the service rather than creating
//! it, so the CLI publishes until the subscriber has attached.
//!
//! The typed sibling runs the other direction: a typed `#[repr(C)]`
//! publisher owns the service first, and `listen_iceoryx` must open it with
//! the live payload type (not `[u8]`) and decode it against a flat layout.
//!
//! `#[ignore]`'d for shmem-restricted environments.

mod common;
use common::sb_bin;

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use iceoryx2::prelude::*;
use sb_listen::{
    DecodeContext, FrameIter, FramePayload, Iox2Field, Iox2Lookup, Iox2Scalar, Iox2Struct,
    Iox2Type, decode_frame_dynamic, listen_iceoryx,
};

#[test]
#[ignore = "live iceoryx2 — run with --ignored"]
fn sb_topic_pub_round_trips_through_iceoryx2() {
    // Fully-qualified topic — iox2 service names allow `/`-prefixed
    // paths, but the CLI trims the leading `/` before opening, so both
    // forms target the same service. Use the fully-qualified form to
    // mirror the user-facing call.
    let topic_full = "/test/lvl4/roundtrip/iox2/payload";
    let (tx, rx) = mpsc::channel();
    let sub_handle = thread::spawn(move || {
        // Blocks until `sb topic pub` has created the `[u8]` service, then
        // opens it with that payload type.
        let mut sub = match listen_iceoryx(topic_full) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Err(format!("listen_iceoryx: {e}")));
                return;
            }
        };
        match sub.next_frame() {
            Ok(frame) => {
                let _ = tx.send(Ok(frame));
            }
            Err(e) => {
                let _ = tx.send(Err(format!("next_frame: {e}")));
            }
        }
    });

    // A one-shot publish only reaches a subscriber that has already
    // attached, and the subscriber can only attach once some publish has
    // created the service (it polls the registry every 200 ms). The first
    // publish may therefore go unseen; repeat until a frame arrives.
    let mut received = None;
    for _ in 0..10 {
        // NOTE: use the real HOME / iox2 system root so the child publisher
        // shares the same shared-memory namespace as the in-process
        // subscriber thread. The standard `Sandbox::cmd()` helper overrides
        // HOME, which makes iox2 use a different per-user shm directory and
        // the publisher's samples become invisible to the subscriber.
        let out = Command::new(sb_bin())
            .args([
                "topic",
                "pub",
                "--transport",
                "iceoryx2",
                topic_full,
                "deadbeef",
            ])
            .output()
            .expect("sb topic pub");
        assert!(
            out.status.success(),
            "sb topic pub failed: stdout={:?} stderr={:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if let Ok(result) = rx.recv_timeout(Duration::from_millis(500)) {
            received = Some(result);
            break;
        }
    }
    let frame = received
        .expect("subscriber did not receive a frame after 10 publishes")
        .expect("subscriber errored");
    let _ = sub_handle.join();

    assert_eq!(frame.map(|f| f.bytes), Some(vec![0xde, 0xad, 0xbe, 0xef]));
}

/// Flat payload shaped like sb-iox2-typegen output: 4 bytes of natural
/// padding after `seq`, a string, and a `bytes` field with its `_len`
/// companion. No `#[type_name]`, so iceoryx2 registers the Rust
/// `type_name` path and the listener has to map it back by leaf.
#[derive(Debug, Clone, Copy, ZeroCopySend)]
#[repr(C)]
struct MiniStamped {
    seq: u32,
    stamp: f64,
    label: [u8; 16],
    data: [u8; 8],
    data_len: u32,
}

/// The vault layout matching [`MiniStamped`] field for field.
fn mini_layout(qualified_name: &str, label_capacity: usize) -> Iox2Struct {
    let field = |name: &str, ty| Iox2Field {
        name: name.into(),
        ty,
    };
    Iox2Struct {
        qualified_name: qualified_name.into(),
        leaf: "MiniStamped".into(),
        fields: vec![
            field("seq", Iox2Type::Scalar(Iox2Scalar::U32)),
            field("stamp", Iox2Type::Scalar(Iox2Scalar::F64)),
            field(
                "label",
                Iox2Type::String {
                    capacity: label_capacity,
                },
            ),
            field("data", Iox2Type::Bytes { capacity: 8 }),
            field("data_len", Iox2Type::Scalar(Iox2Scalar::U32)),
        ],
    }
}

#[test]
#[ignore = "live iceoryx2 — run with --ignored"]
fn listen_iceoryx_opens_typed_service_and_decodes_it() {
    let name = "test/lvl4/listen_typed/iox2/mini";
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let pub_handle = thread::spawn(move || {
        let node = NodeBuilder::new()
            .create::<ipc::Service>()
            .expect("iox2 node");
        let service = node
            .service_builder(&ServiceName::new(name).expect("service name"))
            .publish_subscribe::<MiniStamped>()
            .open_or_create()
            .expect("typed service");
        let publisher = service.publisher_builder().create().expect("publisher");
        ready_tx.send(()).ok();
        let mut label = [0u8; 16];
        label[..5].copy_from_slice(b"hello");
        let msg = MiniStamped {
            seq: 7,
            stamp: 1.5,
            label,
            data: [1, 2, 3, 0, 0, 0, 0, 0],
            data_len: 3,
        };
        while !stop_t.load(Ordering::Relaxed) {
            let sample = publisher.loan_uninit().expect("loan");
            sample.write_payload(msg).send().expect("send");
            thread::sleep(Duration::from_millis(20));
        }
    });
    ready_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("typed publisher ready");

    // The service exists with a typed payload, so this must not fall back
    // to `[u8]` (which would fail with IncompatibleTypes).
    let mut sub = listen_iceoryx(name).expect("listen_iceoryx on a typed service");
    let payload = sub.payload_detail().clone();
    assert_eq!(payload.size, std::mem::size_of::<MiniStamped>());
    assert!(!payload.is_slice);
    let received = sub.next_frame().expect("next_frame").expect("one frame");
    stop.store(true, Ordering::Relaxed);
    let _ = pub_handle.join();
    assert_eq!(received.bytes.len(), payload.size);
    assert_eq!(
        received.wire_schema.as_deref(),
        Some(payload.type_name.as_str())
    );

    // Same leaf twice; only the right one has the live payload's size.
    let layouts = [
        mini_layout("other/lvl4/MiniStamped", 32),
        mini_layout("test/lvl4/MiniStamped", 16),
    ];
    let mut ctx = DecodeContext {
        pool: None,
        iox2: Iox2Lookup {
            structs: &layouts,
            payload: Some(&payload),
        },
        no_data: false,
        keep_bytes: false,
    };
    let frame = decode_frame_dynamic("iceoryx2", name, &received, false, None, &ctx);
    assert_eq!(frame.schema.as_deref(), Some("test/lvl4/MiniStamped"));
    assert_eq!(frame.schema_source, Some("wire"));
    assert_eq!(frame.encoding, Some("iox2"));
    let json = match &frame.payload {
        FramePayload::Json { json } => json.clone(),
        other => panic!("expected Json payload, got {other:?}"),
    };
    assert_eq!(json["seq"], 7, "{json}");
    assert_eq!(json["stamp"], 1.5, "{json}");
    assert_eq!(json["label"], "hello", "{json}");
    assert_eq!(json["data"], serde_json::json!([1, 2, 3]), "{json}");
    assert_eq!(json["data_len"], 3, "{json}");

    ctx.no_data = true;
    let frame = decode_frame_dynamic("iceoryx2", name, &received, false, None, &ctx);
    let FramePayload::Json { json } = &frame.payload else {
        panic!("expected Json payload, got {:?}", frame.payload);
    };
    assert!(json.get("data").is_none(), "--no-data keeps data: {json}");
    assert_eq!(json["data_len"], 3, "{json}");
}
