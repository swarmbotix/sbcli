//! `sb topic listen` must decode a message whose proto imports another
//! proto in the same style.
//!
//! Regression for the ws-ips report: the pool builder ran protoc with the
//! vault root as the include path, so `import "custom/A.proto"` resolved
//! against `<root>/` instead of `<root>/<style>/message_definitions/`,
//! failed, and disabled decoding for every type in the vault. The pool
//! is now built per style through [`sb_listen::build_pool`].
//!
//! Needs real protoc; skips when it is not configured.

mod common;

use common::{Sandbox, protoc_available, protoc_path};

#[test]
fn pool_resolves_message_that_imports_sibling_proto() {
    let s = Sandbox::new();
    if !protoc_available(&s) {
        eprintln!("protoc not configured on this host — skipping");
        return;
    }
    let defs = s.vault_dir().join("custom");
    std::fs::create_dir_all(&defs).unwrap();
    std::fs::write(
        defs.join("A.proto"),
        "syntax = \"proto3\";\npackage swarmbotix.custom;\nmessage Vec3 { float x = 1; }\n",
    )
    .unwrap();
    std::fs::write(
        defs.join("B.proto"),
        "syntax = \"proto3\";\npackage swarmbotix.custom;\nimport \"custom/A.proto\";\nmessage Pose { Vec3 p = 1; }\n",
    )
    .unwrap();

    let vault = sb_vault::Vault::at(s.messages_root());
    let pool = sb_listen::build_pool(&vault, &protoc_path(&s).unwrap()).expect("pool builds");

    assert!(pool.get_message_by_name("swarmbotix.custom.Pose").is_some());
    assert!(pool.get_message_by_name("swarmbotix.custom.Vec3").is_some());
}
