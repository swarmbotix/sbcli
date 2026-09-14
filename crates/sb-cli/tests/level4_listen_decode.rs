//! L4 TDD #7 — `sb topic listen` against a real protobuf `*Stamped`
//! publisher emits JSON-decoded payload, not hex.
//!
//! Compiles a real FileDescriptorSet via the L1 vault, spawns a Zenoh
//! publisher in a background thread that emits proto-encoded
//! `std/StringStamped` (Header + `data: "hello-from-test"`), and calls
//! [`sb_listen::decode_frame_dynamic`] directly. This avoids spawning a
//! subprocess for the unit-of-decode contract — the end-to-end CLI path
//! is covered by [level4_e2e_rust_stamped.rs](level4_e2e_rust_stamped.rs).
//!
//! `#[ignore]`'d because building the pool runs `protoc`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use prost::Message;
use prost_reflect::DescriptorPool;
use sb_discover::{HeaderProbe, MetadataProbe, StampedProbe};
use sb_listen::{FrameIter, FramePayload, decode_frame_dynamic, listen_zenoh};
use zenoh::Wait;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("CARGO_MANIFEST_DIR has a great-grandparent")
        .to_path_buf()
}

fn protoc_path() -> PathBuf {
    let dev_box = PathBuf::from("/opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc");
    if dev_box.exists() {
        return dev_box;
    }
    which::which("protoc").expect("protoc on PATH (test requires it)")
}

fn build_pool() -> DescriptorPool {
    let vault = sb_vault::Vault::at(repo_root().join("messages"));
    // `std/` ships in the ros2 style, and protoc gets exactly that style's
    // include root — see messages/README.md.
    let defs = vault.defs_dir("ros2");
    let names = vault.list_style("ros2").expect("vault list");
    assert!(
        !names.is_empty(),
        "ros2 style should contain bundled std/*.proto"
    );
    let sources: Vec<PathBuf> = names.iter().map(|n| vault.path_of(n)).collect();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    sb_vault::compile_to_descriptor_set(&protoc_path(), &defs, &sources, tmp.path())
        .expect("protoc compile");
    let bytes = std::fs::read(tmp.path()).unwrap();
    DescriptorPool::decode(bytes.as_slice()).expect("DescriptorPool::decode")
}

#[test]
#[ignore = "live zenoh + protoc — run with --ignored"]
fn listen_dynamic_decodes_stringstamped_data_field_to_json() {
    let key = "test/lvl4/listen_decode/string_stamped";
    let pool = build_pool();

    // Publisher thread — emits a real proto StringStamped.
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let pub_handle = thread::spawn(move || {
        let session = zenoh::open(zenoh::Config::default()).wait().unwrap();
        let pubr = session.declare_publisher(key).wait().unwrap();
        // Encode StringStamped manually — same wire layout the
        // example's prost-generated types emit (Header at field 1,
        // string `data` at field 2). Use sb-discover's HeaderProbe
        // for the Header half and append the `data` field by hand so
        // this test doesn't need its own build.rs.
        let header = HeaderProbe {
            timestamp_ns: 1_700_000_000_000_000_000,
            frame_id: "world".into(),
            metadata: Some(MetadataProbe {
                topic_full_name: format!("/{}", key),
                msg_type: "ros2/std/StringStamped".into(),
                msg_freq_desired: 10.0,
            }),
        };
        // Outer StringStamped: field 1 Header (len-delimited), field 2 string `data`.
        let mut wire = Vec::new();
        let mut header_buf = Vec::new();
        StampedProbe {
            header: Some(header),
        }
        .encode(&mut header_buf)
        .unwrap();
        // StampedProbe already prefixes Header with `tag 1 + len`, so its
        // bytes ARE field 1. Append field 2 = string "hello-from-test".
        wire.extend_from_slice(&header_buf);
        wire.push(0x12); // tag 2 << 3 | wire type 2 (length-delimited)
        let data = b"hello-from-test";
        prost::encoding::encode_varint(data.len() as u64, &mut wire);
        wire.extend_from_slice(data);

        while !stop_t.load(Ordering::Relaxed) {
            let _ = pubr.put(wire.clone()).wait();
            thread::sleep(Duration::from_millis(100));
        }
        let _ = pubr.undeclare().wait();
        let _ = session.close().wait();
    });

    thread::sleep(Duration::from_millis(300));

    // Listen for one frame.
    let mut sub = listen_zenoh(key).expect("listen_zenoh");
    let bytes = sub
        .next_frame()
        .expect("next_frame")
        .expect("subscriber should receive at least one sample");
    stop.store(true, Ordering::Relaxed);
    let _ = pub_handle.join();

    let frame = decode_frame_dynamic("zenoh", key, &bytes, false, &pool);
    assert_eq!(frame.schema.as_deref(), Some("ros2/std/StringStamped"));
    assert_eq!(frame.encoding, Some("protobuf"));
    let json = match &frame.payload {
        FramePayload::Json { json } => json.clone(),
        other => panic!("expected Json payload, got {other:?}"),
    };
    assert_eq!(
        json["data"], "hello-from-test",
        "decoded payload should expose `data` field: {json}"
    );
    // Header fields should also be decoded since prost-reflect walks
    // the full message recursively.
    let meta = &json["header"]["metadata"];
    assert_eq!(
        meta["msgType"], "ros2/std/StringStamped",
        "decoded metadata: {json}"
    );
    assert_eq!(meta["msgFreqDesired"], 10.0);
}

#[test]
#[ignore = "live zenoh + protoc — run with --ignored"]
fn listen_dynamic_falls_back_to_hex_for_foreign_bytes() {
    let key = "test/lvl4/listen_decode/foreign";
    let pool = build_pool();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let h = thread::spawn(move || {
        let session = zenoh::open(zenoh::Config::default()).wait().unwrap();
        let pubr = session.declare_publisher(key).wait().unwrap();
        while !stop_t.load(Ordering::Relaxed) {
            let _ = pubr.put(b"HELLO_RAW".to_vec()).wait();
            thread::sleep(Duration::from_millis(100));
        }
        let _ = pubr.undeclare().wait();
        let _ = session.close().wait();
    });
    thread::sleep(Duration::from_millis(300));
    let mut sub = listen_zenoh(key).expect("listen_zenoh");
    let bytes = sub.next_frame().unwrap().unwrap();
    stop.store(true, Ordering::Relaxed);
    let _ = h.join();
    let frame = decode_frame_dynamic("zenoh", key, &bytes, false, &pool);
    assert!(
        frame.schema.is_none(),
        "foreign bytes should leave schema null"
    );
    match &frame.payload {
        FramePayload::Hex { .. } => {}
        other => panic!("expected Hex fallback, got {other:?}"),
    }
}
