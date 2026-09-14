//! L2 TDD test #4 — `sb init` is idempotent.

mod common;
use common::WsSandbox;

fn mtime(p: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(p).unwrap().modified().unwrap()
}

#[test]
fn rerunning_init_is_no_op() {
    let s = WsSandbox::new();
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();

    let project = s.tempdir().join("pubber");
    std::fs::create_dir_all(&project).unwrap();

    // First call.
    let out = s
        .cmd()
        .args(["init", "--rust"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "first init failed: {out:?}");

    let dev_yml = project.join("sb.dev.yml");
    let rs = project.join("runscript.bash");
    let flow = s
        .sb_home()
        .join("workspaces")
        .join("demo")
        .join("flow.yaml");

    let mtime_dev_1 = mtime(&dev_yml);
    let mtime_rs_1 = mtime(&rs);
    let flow_1 = std::fs::read_to_string(&flow).unwrap();

    // Sleep long enough for mtime resolution to register any rewrite.
    std::thread::sleep(std::time::Duration::from_millis(20));

    // Second call: same args.
    let out = s
        .cmd()
        .args(["init", "--rust"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "second init failed: {out:?}");

    assert_eq!(mtime(&dev_yml), mtime_dev_1, "sb.dev.yml rewritten");
    assert_eq!(mtime(&rs), mtime_rs_1, "runscript.bash rewritten");
    assert_eq!(
        std::fs::read_to_string(&flow).unwrap(),
        flow_1,
        "flow.yaml changed"
    );

    // Third call: no language flag (must succeed; language already on disk).
    let out = s.cmd().args(["init"]).arg(&project).output().unwrap();
    assert!(out.status.success(), "third init (no flag) failed: {out:?}");
    assert_eq!(
        mtime(&dev_yml),
        mtime_dev_1,
        "third call rewrote sb.dev.yml"
    );
    assert_eq!(
        std::fs::read_to_string(&flow).unwrap(),
        flow_1,
        "third call changed flow.yaml"
    );

    // flow.yaml has exactly one pubber entry.
    let occurrences = flow_1.matches("pubber:").count();
    assert_eq!(occurrences, 1, "duplicate pubber entry: {flow_1}");
}
