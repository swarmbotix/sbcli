//! `runscript.bash` template emitted by `sb init`.
//!
//! Two template flavors:
//!
//! * **Stub** (default) — language-agnostic shell wrapper with a
//!   commented example line per supported language. Body exits non-zero
//!   so `sb run` / `sb up` fail loudly until the user edits in their
//!   launch command. Used by `sb init` without `--docker`.
//!
//! * **Docker** (`sb init --docker`) — pre-resolved `docker run`
//!   invocation per instance declared in `flow.yaml::instances`.
//!   `--id <name>` is required; the case-branch picks the matching
//!   instance's image / env / device / mount / gpu / network flags.
//!   See `documents/sbcli_docker_runscript.md` for per-language
//!   in-container entry-point sketches.
//!
//! Test #8 in [level2.html](../../plan/level2.html) golden-snippets
//! the stub comment block, so changes to the stub are intentional.

use std::path::Path;

use sb_core::ModuleInstance;

const TEMPLATE: &str = r#"#!/usr/bin/env bash
# {{RUNSCRIPT_PATH}}
# Invoked by `sb run` / `sb up`, or directly from any cwd. The `cd`
# below pins the working dir to this script's directory (= module
# root) so the commands below always run from the right place.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

# Standalone tmux wrap: when this script is run directly (not from
# `sb up`, which already runs us inside its own tmux pane), re-exec
# inside a per-instance tmux session so the app is detachable and its
# output persists. `-A` attaches if the session is already running.
#
# Session name is `{{MODULE}}-dev` for the no-id case, and
# `{{MODULE}}-<id>-dev` when `--id <id>` is in "$@". This is what
# makes `bash runscript.bash --id 1` and `bash runscript.bash --id 2`
# launch as TWO independent processes on two distinct wire paths
# (`/.../{{MODULE}}-1/...` and `/.../{{MODULE}}-2/...`) rather than
# the second call silently reattaching to the first's session — which
# would discard the new tmux command and only ever run one binary.
# See documents/sbcli_pubsub_consuming.md §10 for the swarm pattern.
#
# After the launch command exits we pause for a keypress so build
# errors / crash output stay readable — tmux otherwise closes the
# pane the instant the inner bash exits and you'd never see why.
# The outer wrapper traps SIGINT so Ctrl+C inside the pane stops the
# app but does NOT kill the wrapper itself (otherwise the pane closes
# before you can read the exit message). The inner subshell resets
# the trap so the app receives Ctrl+C normally.
# Set `SB_NO_TMUX=1` to skip the wrap (CI, scripts, IDE runners).
# If tmux isn't on PATH, we fall through to inline.
if [[ -z "${TMUX:-}" ]] && [[ "${SB_NO_TMUX:-0}" != "1" ]] && command -v tmux >/dev/null 2>&1; then
  __sb_self="$PWD/$(basename "${BASH_SOURCE[0]}")"
  __sb_args=""
  for __arg in "$@"; do __sb_args="$__sb_args $(printf %q "$__arg")"; done
  __sb_id_suffix=""
  __sb_prev=""
  for __arg in "$@"; do
    if [[ "$__sb_prev" == "--id" ]]; then __sb_id_suffix="-$__arg"; break; fi
    __sb_prev="$__arg"
  done
  __sb_session="{{MODULE}}${__sb_id_suffix}-dev"
  exec tmux new -A -s "${__sb_session}" \
    "trap '' INT; (trap - INT; bash $(printf %q "${__sb_self}")${__sb_args}); ec=\$?; echo; echo \"[app exited \$ec — press any key to close pane]\"; read -n 1"
fi

# All "$@" args passed to this script are forwarded to the inner app.
# Use this to thread per-instance overrides through to your `main()` —
# e.g., `bash runscript.bash --id 2` lets your main parse `--id 2` and
# construct the generated publisher with `module = "<MODULE>-2"`, which
# shifts the wire path to `<device>/<ws>/<MODULE>-2/<transport>/<topic>`.
# The tmux-wrap above already extracts the same `--id` value so each
# instance lands in its own tmux session (`{{MODULE}}-2-dev`) — that
# way N invocations from N terminals truly run N processes side by
# side. See documents/sbcli_pubsub_consuming.md for per-language sketches.

# Replace the TODO block at the bottom with the command(s) that build
# and start this module. Uncomment one of the examples below — each is
# a runnable bash line — or write your own. End with `exec <cmd> "$@"`
# so per-instance flags reach your `main()`.

# cargo run --release -- "$@"                       # Rust
# python main.py "$@"                               # Python
# cmake --build build && exec ./build/app "$@"      # C++
# flutter run -d linux                              # Flutter
# : 'no command — Unity launches from the editor'   # Unity

echo "TODO: edit {{RUNSCRIPT_PATH}} to launch this module" >&2
exit 1
"#;

/// Render the stub template with the absolute path of the emitted file
/// and the module name substituted in. Called by `init_module` once
/// it has resolved the final runscript path and the module's
/// identifier (validated `basename(root)`).
pub fn render(runscript_path: &Path, module: &str) -> String {
    TEMPLATE
        .replace("{{RUNSCRIPT_PATH}}", &runscript_path.display().to_string())
        .replace("{{MODULE}}", module)
}

// ─────────────────────────────────────────────────────────────────────
// Docker runscript template — `sb init --docker`
// ─────────────────────────────────────────────────────────────────────

/// Render a docker-flavored runscript. Every instance in `instances`
/// becomes one `case` branch that resolves to a fully-formed
/// `docker run … <image> "$@"` invocation. Per-instance docker args
/// (image, env, devices, mounts, gpus, network) are baked in at
/// render time — the runscript performs no flow.yaml lookup at runtime
/// and has no dependency on `sb` being on PATH.
///
/// `flow_yaml_path` is embedded into a staleness check: if the user
/// edits `flow.yaml::instances` after this script is generated, the
/// next invocation emits a one-line warning to stderr (the script
/// still runs, on the assumption that the existing branches are good
/// enough; users regenerate with `sb init --docker --force`).
// `push_str(&format!(..))` throughout: this is a template builder that runs
// once per `sb init --docker`, and reading it as a linear transcript of the
// emitted script matters more than sparing an intermediate String.
#[allow(clippy::format_push_string)]
pub fn render_docker(
    runscript_path: &Path,
    module: &str,
    instances: &[ModuleInstance],
    flow_yaml_path: &Path,
) -> String {
    let mut branches = String::new();
    let mut known: Vec<String> = Vec::with_capacity(instances.len());
    for inst in instances {
        known.push(inst.id.clone());
        branches.push_str(&render_case_branch(module, inst));
    }
    let known_list = known.join(" ");

    let mut out = String::new();
    out.push_str("#!/usr/bin/env bash\n");
    out.push_str(&format!("# {}\n", runscript_path.display()));
    out.push_str("# regenerate with: sb init --docker --force\n");
    out.push_str("#\n");
    out.push_str(
        "# Docker-flavored runscript: each `--id <name>` selects one pre-resolved\n\
         # `docker run` invocation below. Per-instance image, env, --device, mounts,\n\
         # --gpus, and --network were baked in from flow.yaml::instances at\n\
         # `sb init --docker` time. Edit flow.yaml and re-run\n\
         # `sb init --docker --force` to update.\n",
    );
    out.push_str("set -euo pipefail\n");
    out.push_str("cd -- \"$(dirname -- \"${BASH_SOURCE[0]}\")\"\n\n");

    // Staleness check: warn (don't fail) if flow.yaml is newer than us.
    out.push_str("# Staleness check — warn if flow.yaml has been edited since we were\n");
    out.push_str("# generated. The script still runs; regenerate with --force to refresh.\n");
    out.push_str(&format!(
        "__sb_flow={}\n",
        shell_single_quote(&flow_yaml_path.display().to_string())
    ));
    out.push_str("__sb_self=\"$PWD/$(basename \"${BASH_SOURCE[0]}\")\"\n");
    out.push_str("if [[ -f \"$__sb_flow\" ]] && [[ \"$__sb_flow\" -nt \"$__sb_self\" ]]; then\n");
    out.push_str(
        "  echo \"warning: $__sb_flow is newer than $__sb_self — \
         instances may be stale; regenerate with: sb init --docker --force\" >&2\n",
    );
    out.push_str("fi\n\n");

    // --id parser.
    out.push_str("# Parse --id <name>. Required for the docker variant — there is no\n");
    out.push_str("# default branch because every instance carries its own container args.\n");
    out.push_str("__sb_id=\"\"\n");
    out.push_str("__sb_prev=\"\"\n");
    out.push_str("for __arg in \"$@\"; do\n");
    out.push_str("  if [[ \"$__sb_prev\" == \"--id\" ]]; then __sb_id=\"$__arg\"; break; fi\n");
    out.push_str("  __sb_prev=\"$__arg\"\n");
    out.push_str("done\n");
    out.push_str("if [[ -z \"$__sb_id\" ]]; then\n");
    out.push_str(&format!(
        "  echo \"error: --id <name> required (known: {known_list})\" >&2\n"
    ));
    out.push_str("  exit 2\n");
    out.push_str("fi\n\n");

    out.push_str("case \"$__sb_id\" in\n");
    out.push_str(&branches);
    out.push_str("  *)\n");
    out.push_str(&format!(
        "    echo \"error: unknown --id '$__sb_id' (known: {known_list})\" >&2\n"
    ));
    out.push_str("    exit 2\n");
    out.push_str("    ;;\n");
    out.push_str("esac\n");
    out
}

// See `render_docker`: same template-builder rationale.
#[allow(clippy::format_push_string)]
fn render_case_branch(module: &str, inst: &ModuleInstance) -> String {
    let docker = inst
        .docker
        .as_ref()
        .expect("preflight guarantees every instance has a docker spec");
    let image = docker
        .image
        .as_deref()
        .expect("preflight guarantees every docker spec has an image");

    let mut out = String::new();
    out.push_str(&format!("  {})\n", shell_single_quote(&inst.id)));
    out.push_str("    exec docker run --rm -it \\\n");
    out.push_str(&format!(
        "      --name {} \\\n",
        shell_single_quote(&format!("{module}-{}", inst.id))
    ));
    // `--hostname` matches `--name` so processes inside see the
    // instance identity; useful for logging and unique per-process
    // service names that the in-container code might derive from
    // `hostname`.
    out.push_str(&format!(
        "      --hostname {} \\\n",
        shell_single_quote(&format!("{module}-{}", inst.id))
    ));
    for (k, v) in &docker.env {
        // env(1) format: KEY=VALUE; the whole pair is one shell-quoted argv.
        let pair = format!("{k}={v}");
        out.push_str(&format!("      -e {} \\\n", shell_single_quote(&pair)));
    }
    for dev in &docker.devices {
        out.push_str(&format!("      --device {} \\\n", shell_single_quote(dev)));
    }
    for mnt in &docker.mounts {
        out.push_str(&format!("      -v {} \\\n", shell_single_quote(mnt)));
    }
    if let Some(gpus) = &docker.gpus {
        out.push_str(&format!("      --gpus {} \\\n", shell_single_quote(gpus)));
    }
    if let Some(net) = &docker.network {
        out.push_str(&format!("      --network {} \\\n", shell_single_quote(net)));
    }
    // Static args from flow.yaml::instances[].args follow the image,
    // before "$@" so caller-supplied args take precedence.
    out.push_str(&format!("      {}", shell_single_quote(image)));
    for arg in &inst.args {
        out.push_str(&format!(" {}", shell_single_quote(arg)));
    }
    out.push_str(" \"$@\"\n");
    out.push_str("    ;;\n");
    out
}

/// Wrap `s` in single quotes for safe use as a bash argv element.
/// Single quotes can't contain a literal `'`; split with `'\''`.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use sb_core::DockerSpec;

    fn left_right_fixture() -> Vec<ModuleInstance> {
        let mut env_left = BTreeMap::new();
        env_left.insert("CAM_SERIAL".into(), "A123".into());
        env_left.insert("LOG_LEVEL".into(), "info".into());
        let mut env_right = BTreeMap::new();
        env_right.insert("CAM_SERIAL".into(), "B456".into());
        vec![
            ModuleInstance {
                id: "left".into(),
                label: Some("Left Camera".into()),
                docker: Some(DockerSpec {
                    image: Some("cam_gige_ht:latest".into()),
                    env: env_left,
                    devices: vec!["/dev/video0".into()],
                    mounts: vec![],
                    gpus: Some("all".into()),
                    network: None,
                }),
                args: vec![],
            },
            ModuleInstance {
                id: "right".into(),
                label: None,
                docker: Some(DockerSpec {
                    image: Some("cam_gige_ht:latest".into()),
                    env: env_right,
                    devices: vec!["/dev/video1".into()],
                    mounts: vec![],
                    gpus: None,
                    network: None,
                }),
                args: vec![],
            },
        ]
    }

    #[test]
    fn docker_runscript_includes_branch_per_instance() {
        let rs = render_docker(
            &PathBuf::from("/m/runscript.bash"),
            "cam_gige_ht",
            &left_right_fixture(),
            &PathBuf::from("/ws/flow.yaml"),
        );
        // Each id appears as a case-branch label.
        assert!(rs.contains("'left')"), "missing left branch:\n{rs}");
        assert!(rs.contains("'right')"), "missing right branch:\n{rs}");
        // Container name = <module>-<id>.
        assert!(rs.contains("--name 'cam_gige_ht-left'"), "name missing");
        assert!(rs.contains("--name 'cam_gige_ht-right'"), "name missing");
        // Env, devices, gpus baked in.
        assert!(rs.contains("'CAM_SERIAL=A123'"));
        assert!(rs.contains("'CAM_SERIAL=B456'"));
        assert!(rs.contains("--device '/dev/video0'"));
        assert!(rs.contains("--device '/dev/video1'"));
        assert!(rs.contains("--gpus 'all'"));
        // Image, then "$@".
        assert!(
            rs.contains("'cam_gige_ht:latest' \"$@\""),
            "image+args trail missing"
        );
        // Header is the docker variant, not the stub.
        assert!(rs.contains("regenerate with: sb init --docker --force"));
        assert!(rs.contains("/ws/flow.yaml"), "flow.yaml path embedded");
        // Unknown-id fallback names known instances.
        assert!(rs.contains("known: left right"));
    }

    #[test]
    fn shell_single_quote_escapes_internal_apostrophe() {
        assert_eq!(shell_single_quote("a"), "'a'");
        assert_eq!(shell_single_quote("a'b"), r"'a'\''b'");
        assert_eq!(shell_single_quote(""), "''");
    }
}
